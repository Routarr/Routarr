#!/usr/bin/env python3
"""An OpenID Connect provider stand-in for the end-to-end suite.

A real server the release binary can reach, as fake_arr.py is, beside the
in-process one of src/tests/fake_oidc.rs. It signs in whoever reaches its
authorization endpoint, as a provider signs in the person at the keyboard, and
gives a token only for a code it issued, to the client it was issued for, with
the verifier its challenge was made from.

The ID token is unsigned: Routarr checks its claims and not its signature,
since the token arrives on the connection it opened to the token endpoint.
"""

import base64
import hashlib
import json
import os
import secrets
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlencode, urlsplit

PORT = int(os.environ.get("OIDC_PORT", "7980"))
ISSUER = f"http://127.0.0.1:{PORT}"
CLIENT_ID = os.environ["OIDC_CLIENT_ID"]
CLIENT_SECRET = os.environ["OIDC_CLIENT_SECRET"]
SUBJECT = "e2e-operator"

# Each code is spent by the one token request it answers.
ISSUED = {}


def b64url(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def single(values):
    return {name: value[0] for name, value in values.items()}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, payload, status=200):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        url = urlsplit(self.path)
        if url.path == "/.well-known/openid-configuration":
            return self._send(
                {
                    "issuer": ISSUER,
                    "authorization_endpoint": f"{ISSUER}/authorize",
                    "token_endpoint": f"{ISSUER}/token",
                }
            )
        if url.path == "/authorize":
            asked = single(parse_qs(url.query))
            if asked.get("client_id") != CLIENT_ID or asked.get("code_challenge_method") != "S256":
                return self._send({"error": "invalid_request"}, 400)
            code = secrets.token_urlsafe(16)
            ISSUED[code] = asked
            back = urlencode({"code": code, "state": asked["state"]})
            self.send_response(302)
            self.send_header("Location", f"{asked['redirect_uri']}?{back}")
            self.end_headers()
            return None
        return self._send({}, 404)

    def do_POST(self):
        if urlsplit(self.path).path != "/token":
            return self._send({}, 404)
        length = int(self.headers.get("Content-Length", 0))
        form = single(parse_qs(self.rfile.read(length).decode()))
        asked = ISSUED.pop(form.get("code", ""), None)
        verifier = form.get("code_verifier", "").encode()
        if (
            asked is None
            or form.get("client_id") != CLIENT_ID
            or form.get("client_secret") != CLIENT_SECRET
            or form.get("redirect_uri") != asked["redirect_uri"]
            or b64url(hashlib.sha256(verifier).digest()) != asked.get("code_challenge")
        ):
            return self._send({"error": "invalid_grant"}, 400)
        now = int(time.time())
        claims = {
            "iss": ISSUER,
            "sub": SUBJECT,
            "aud": CLIENT_ID,
            "iat": now,
            "exp": now + 300,
            "nonce": asked.get("nonce"),
        }
        header = b64url(json.dumps({"alg": "none", "typ": "JWT"}).encode())
        payload = b64url(json.dumps(claims).encode())
        return self._send(
            {
                "access_token": secrets.token_urlsafe(16),
                "token_type": "Bearer",
                "expires_in": 300,
                "id_token": f"{header}.{payload}.",
            }
        )


if __name__ == "__main__":
    HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()

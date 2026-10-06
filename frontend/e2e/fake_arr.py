#!/usr/bin/env python3
"""A Radarr stand-in for the end-to-end suite.

Deliberately separate from backend/src/tests/fake_arr.rs: that one lives inside
the Rust test process, this one has to be a real server the release binary can
reach.

State is in memory and mutated by each film's update, so a test can assert
that a move really reached "Radarr" rather than only that Routarr thinks it
did.
"""

import json
import os
from http.server import BaseHTTPRequestHandler, HTTPServer

def initial_root_folders():
    return [
        {"id": 1, "path": "/movies/standard", "freeSpace": 900_000_000_000, "accessible": True},
        {"id": 2, "path": "/movies/anime", "freeSpace": 400_000_000_000, "accessible": True},
    ]


ROOT_FOLDERS = initial_root_folders()


def movie(
    arr_id,
    title,
    year,
    tmdb_id,
    has_file=True,
    root="/movies/standard",
    genres=None,
):
    return {
        "id": arr_id,
        "title": title,
        "sortTitle": title.lower(),
        "year": year,
        "tmdbId": tmdb_id,
        "imdbId": None,
        "path": f"{root}/{title} ({year})",
        "rootFolderPath": root,
        "monitored": True,
        "hasFile": has_file,
        "status": "released",
        "added": "2026-08-19T10:00:00Z",
        # Radarr reports these itself. They are what lets Routarr classify with
        # no TMDb key, which is how the end-to-end stack runs.
        "genres": genres if genres is not None else ["Drama"],
        "originalLanguage": {"id": 1, "name": "English"},
        "certification": "PG-13",
    }


def initial_movies():
    return [
        movie(1, "My Neighbor Totoro", 1988, 8392, genres=["Animation", "Family"]),
        movie(2, "Akira", 1988, 149, genres=["Animation", "Science Fiction"]),
        movie(3, "The Matrix", 1999, 603, genres=["Action", "Science Fiction"]),
        # Added but not downloaded: the case automatic application exists for.
        movie(4, "Perfect Blue", 1997, 10494, has_file=False, genres=["Animation"]),
    ]


MOVIES = initial_movies()

# Each film's update as Radarr received it: whether the files moved on disk is
# only in the request, never in the movie it leaves behind.
EDITS = []


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

    def _body(self):
        length = int(self.headers.get("Content-Length", 0))
        return json.loads(self.rfile.read(length) or b"{}")

    def do_GET(self):
        path = self.path.split("?")[0]
        held = held_movie(path)
        if held is not None:
            return self._send(held)
        # Moves end at once here, and Radarr stops listing an ended command
        # after a few minutes: an empty list reads the same.
        if path == "/api/v3/command":
            return self._send([])
        if path == "/api/v3/system/status":
            return self._send({"version": "5.14.0.9383", "appName": "Radarr"})
        if path == "/api/v3/rootfolder":
            return self._send(ROOT_FOLDERS)
        if path == "/api/v3/movie":
            return self._send(MOVIES)
        # A reset hook, so a test can undo what a previous one moved.
        if path == "/__reset":
            MOVIES[:] = initial_movies()
            EDITS.clear()
            ROOT_FOLDERS[:] = initial_root_folders()
            return self._send({"ok": True})
        if path == "/__edits":
            return self._send(EDITS)
        # A disk that stopped answering, as Radarr reports one: listed, and
        # not accessible.
        if path == "/__asleep":
            asleep = self.path.split("path=", 1)[-1]
            for folder in ROOT_FOLDERS:
                if folder["path"] == asleep:
                    folder["accessible"] = False
            return self._send({"ok": True})
        self._send({}, 404)

    def do_PUT(self):
        path = self.path.split("?")[0]
        item = held_movie(path)
        if item is None:
            return self._send({}, 404)
        payload = self._body()
        move_files = "moveFiles=true" in self.path
        EDITS.append({"movieIds": [item["id"]], "moveFiles": move_files})
        item["rootFolderPath"] = payload["rootFolderPath"]
        item["path"] = payload["path"]
        # Radarr answers 202 with the movie as it now is.
        return self._send(item, 202)


def held_movie(path):
    """The movie `/api/v3/movie/{id}` names, `None` for any other path."""
    prefix = "/api/v3/movie/"
    if not path.startswith(prefix) or not path[len(prefix):].isdigit():
        return None
    arr_id = int(path[len(prefix):])
    return next((item for item in MOVIES if item["id"] == arr_id), None)


if __name__ == "__main__":
    port = int(os.environ.get("ARR_PORT", "7979"))
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()

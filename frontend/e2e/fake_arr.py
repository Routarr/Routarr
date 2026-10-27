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
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlparse


def initial_root_folders():
    return [
        {"id": 1, "path": "/movies/standard", "freeSpace": 900_000_000_000, "accessible": True},
        {"id": 2, "path": "/movies/anime", "freeSpace": 400_000_000_000, "accessible": True},
    ]


ROOT_FOLDERS = initial_root_folders()

TAGS = [{"id": 1, "label": "4k"}, {"id": 2, "label": "rewatch"}]

QUALITY_PROFILES = [{"id": 1, "name": "HD-1080p"}, {"id": 4, "name": "Ultra-HD"}]


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
        # no TMDB key, which is how the end-to-end stack runs.
        "genres": genres if genres is not None else ["Drama"],
        "originalLanguage": {"id": 1, "name": "English"},
        "certification": "PG-13",
        "qualityProfileId": 1,
        "runtime": 120,
        "studio": "A Studio",
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

# Each path asked that this stand-in does not serve. A real Radarr answers it,
# and a 404 here sends Routarr down the path it takes when Radarr fails, which
# the suite would then exercise in place of the ordinary one.
UNKNOWN = []


def best_root_folder(path):
    """The root folder Radarr reports for a title, worked out at every read and never the one
    an edit sent: the longest it lists holding the path, else the folder above it."""
    holding = [f["path"] for f in ROOT_FOLDERS if path.startswith(f["path"].rstrip("/") + "/")]
    return max(holding, key=len) if holding else path.rsplit("/", 1)[0]


def directory_listing(asked):
    """What `/api/v3/filesystem` answers, as Radarr's LookupContents does: the
    query is cut after its last separator and what remains is listed, each
    directory's path ending in a separator."""
    tree = {folder["path"] for folder in ROOT_FOLDERS}
    tree |= {path.rsplit("/", 1)[0] for path in tree}
    if "/" not in asked:
        return {"directories": [], "files": []}
    listed = asked[: asked.rindex("/")]
    children = sorted(child for child in tree if child and child.rsplit("/", 1)[0] == listed)
    return {
        "directories": [
            {"type": "folder", "name": child.rsplit("/", 1)[1], "path": child + "/"}
            for child in children
        ],
        "files": [],
    }


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
        query = parse_qs(urlparse(self.path).query)
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
        # A disk under each root folder, with the free space the folder reports.
        if path == "/api/v3/diskspace":
            return self._send(
                [
                    {
                        "path": folder["path"],
                        "label": "",
                        "freeSpace": folder["freeSpace"],
                        "totalSpace": 4 * folder["freeSpace"],
                    }
                    for folder in ROOT_FOLDERS
                ]
            )
        if path == "/api/v3/movie":
            if "tmdbId" in query:
                tmdb = int(query["tmdbId"][0])
                return self._send([item for item in MOVIES if item["tmdbId"] == tmdb])
            return self._send(MOVIES)
        if path == "/api/v3/movie/lookup/tmdb":
            tmdb = int(query.get("tmdbId", ["0"])[0])
            found = next((item for item in MOVIES if item["tmdbId"] == tmdb), None)
            return self._send(found) if found else self._send({}, 404)
        if path == "/api/v3/movie/lookup/imdb":
            return self._send({}, 404)
        if path == "/api/v3/tag":
            return self._send(TAGS)
        if path == "/api/v3/qualityprofile":
            return self._send(QUALITY_PROFILES)
        if path == "/api/v3/notification":
            return self._send([])
        if path == "/api/v3/config/metadata":
            return self._send({"certificationCountry": "us"})
        if path == "/api/v3/filesystem":
            return self._send(directory_listing(query.get("path", [""])[0] or "/"))
        if path == "/__unknown":
            return self._send(UNKNOWN)
        # A reset hook, so a test can undo what a previous one moved.
        if path == "/__reset":
            MOVIES[:] = initial_movies()
            EDITS.clear()
            ROOT_FOLDERS[:] = initial_root_folders()
            return self._send({"ok": True})
        if path == "/__edits":
            return self._send(EDITS)
        self._unknown("GET", path)

    def do_PUT(self):
        path = self.path.split("?")[0]
        item = held_movie(path)
        if item is None:
            return self._unknown("PUT", path)
        payload = self._body()
        move_files = "moveFiles=true" in self.path
        EDITS.append({"movieIds": [item["id"]], "moveFiles": move_files})
        item["path"] = payload["path"]
        item["rootFolderPath"] = best_root_folder(item["path"])
        # Radarr answers 202 with the movie as it now is.
        return self._send(item, 202)

    def do_POST(self):
        self._unknown("POST", self.path.split("?")[0])

    def _unknown(self, method, path):
        asked = f"{method} {path}"
        if asked not in UNKNOWN:
            UNKNOWN.append(asked)
            print(f"fake Radarr: nothing serves {asked}", file=sys.stderr, flush=True)
        self._send({}, 404)


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

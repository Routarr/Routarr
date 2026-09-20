#!/usr/bin/env python3
"""A Radarr / Sonarr stand-in used only to produce the showcase screenshots.

Separate from `frontend/e2e/fake_arr.py`, which is deliberately minimal: a test
wants the smallest library that proves a behaviour, a screenshot wants one that
looks like somebody's actual collection. Mode and port come from the
environment so one file serves both instances.
"""

import json
import os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlparse

MODE = os.environ.get("ARR_MODE", "radarr")
PORT = int(os.environ.get("ARR_PORT", "7979"))

MOVIE_ROOTS = [
    {"id": 1, "path": "/movies/standard", "freeSpace": 2_400_000_000_000, "accessible": True},
    {"id": 2, "path": "/movies/anime", "freeSpace": 900_000_000_000, "accessible": True},
    {"id": 3, "path": "/movies/kids", "freeSpace": 600_000_000_000, "accessible": True},
    {"id": 4, "path": "/movies/concerts", "freeSpace": 300_000_000_000, "accessible": True},
]

SERIES_ROOTS = [
    {"id": 1, "path": "/tv/standard", "freeSpace": 4_100_000_000_000, "accessible": True},
    {"id": 2, "path": "/tv/anime", "freeSpace": 1_200_000_000_000, "accessible": True},
    {"id": 3, "path": "/tv/documentaries", "freeSpace": 800_000_000_000, "accessible": True},
]

# Every directory the fake filesystem holds: the roots the instance reports
# plus the parents above them, since the client lists a parent to find its leaf.
TREE = sorted(
    {
        segment
        for root in MOVIE_ROOTS + SERIES_ROOTS
        for segment in (
            root["path"],
            root["path"].rsplit("/", 1)[0],
        )
    }
)

TAGS = [{"id": 1, "label": "4k"}, {"id": 2, "label": "rewatch"}, {"id": 3, "label": "imported"}]


# What a real Radarr or Sonarr carries in its payload and Routarr's `arr`
# source reads without a request: without these, every shot showed TMDb
# supplying everything, which the FAQ says is not how it works.
DETAILS = {
    "Akira": (["Animation", "Science Fiction"], "Japanese", "R"),
    "Perfect Blue": (["Animation", "Thriller"], "Japanese", "R"),
    "Spirited Away": (["Animation", "Family", "Fantasy"], "Japanese", "PG"),
    "Your Name": (["Animation", "Romance", "Drama"], "Japanese", "PG"),
    "My Neighbor Totoro": (["Animation", "Family"], "Japanese", "G"),
    "Toy Story": (["Animation", "Family", "Comedy"], "English", "G"),
    "Paddington": (["Family", "Comedy"], "English", "PG"),
    "Stop Making Sense": (["Music", "Documentary"], "English", "PG"),
    "The Last Waltz": (["Music", "Documentary"], "English", "PG"),
    "The Matrix": (["Action", "Science Fiction"], "English", "R"),
    "Blade Runner 2049": (["Science Fiction", "Drama"], "English", "R"),
    "Arrival": (["Science Fiction", "Drama"], "English", "PG-13"),
    "Dune": (["Science Fiction", "Adventure"], "English", "PG-13"),
    "Whiplash": (["Drama", "Music"], "English", "R"),
    "Cowboy Bebop": (["Animation", "Science Fiction"], "Japanese", "TV-14"),
    "Fullmetal Alchemist: Brotherhood": (["Animation", "Adventure"], "Japanese", "TV-14"),
    "Planet Earth II": (["Documentary"], "English", "TV-G"),
    "Chef's Table": (["Documentary"], "English", "TV-MA"),
    "The Expanse": (["Science Fiction", "Drama"], "English", "TV-14"),
    "Severance": (["Drama", "Mystery"], "English", "TV-MA"),
}


def details(title):
    genres, language, certification = DETAILS.get(title, ([], "English", None))
    return {
        "genres": genres,
        "originalLanguage": {"id": 1, "name": language},
        "certification": certification,
    }


def movie(i, title, year, tmdb_id, root="/movies/standard", has_file=True, tags=None):
    return {
        **details(title),
        "id": i,
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
        "sizeOnDisk": 18_000_000_000 if has_file else 0,
        "tags": tags or [],
        "added": "2026-07-14T10:00:00Z",
    }


def series(i, title, year, tvdb_id, tmdb_id, root="/tv/standard", seasons=3, kind="standard"):
    return {
        **details(title),
        "id": i,
        "title": title,
        "sortTitle": title.lower(),
        "year": year,
        "tvdbId": tvdb_id,
        "tmdbId": tmdb_id,
        "imdbId": None,
        "path": f"{root}/{title}",
        "rootFolderPath": root,
        "monitored": True,
        "status": "ended",
        "seriesType": kind,
        "tags": [],
        "added": "2026-06-02T10:00:00Z",
        "seasons": [{"seasonNumber": n} for n in range(0, seasons + 1)],
        "statistics": {
            "episodeFileCount": seasons * 12,
            "episodeCount": seasons * 12,
            "sizeOnDisk": seasons * 40_000_000_000,
        },
    }


MOVIES = [
    movie(1, "Akira", 1988, 149),
    movie(2, "Perfect Blue", 1997, 10494, has_file=False),
    movie(3, "Spirited Away", 2001, 129),
    movie(4, "Your Name", 2016, 372058),
    movie(5, "My Neighbor Totoro", 1988, 8392),
    movie(6, "Toy Story", 1995, 862),
    movie(7, "Paddington", 2014, 116149),
    movie(8, "Stop Making Sense", 1984, 13260),
    movie(9, "The Last Waltz", 1978, 1212),
    movie(10, "The Matrix", 1999, 603, tags=[1]),
    movie(11, "Blade Runner 2049", 2017, 335984, tags=[1]),
    movie(12, "Arrival", 2016, 329865),
    movie(13, "Dune", 2021, 438631, tags=[1]),
    movie(14, "Whiplash", 2014, 244786),
]

SERIES = [
    series(1, "Cowboy Bebop", 1998, 76885, 30991, seasons=1, kind="anime"),
    series(2, "Fullmetal Alchemist: Brotherhood", 2009, 115355, 31911, seasons=1, kind="anime"),
    series(3, "Planet Earth II", 2016, 318408, 68595, seasons=1),
    series(4, "Chef's Table", 2015, 300495, 62974, seasons=6),
    series(5, "The Expanse", 2015, 280619, 63639, seasons=6),
    series(6, "Severance", 2022, 371980, 95396, seasons=2),
]


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
        path = self.path.split("?")[0]
        if path == "/api/v3/system/status":
            return self._send({"version": "5.0.0", "appName": MODE.capitalize()})
        if path == "/api/v3/rootfolder":
            return self._send(MOVIE_ROOTS if MODE == "radarr" else SERIES_ROOTS)
        if path == "/api/v3/filesystem":
            # A listing of the asked-for directory's children, which is what the
            # client reads: it lists a candidate's *parent* and looks for the
            # leaf among `directories`, because this endpoint answers a path it
            # does not know with the nearest directory above it — so a
            # misspelt last segment under a real root comes back looking
            # verified. `parent` is reported the same way here for that reason,
            # and nothing is meant to read it.
            wanted = parse_qs(urlparse(self.path).query).get("path", [""])[0]
            wanted = wanted.rstrip("/") or "/"
            children = sorted(
                {
                    child
                    for child in TREE
                    if child.rsplit("/", 1)[0] == ("" if wanted == "/" else wanted)
                }
            )
            return self._send(
                {
                    "parent": wanted.rsplit("/", 1)[0] or "/",
                    "directories": [
                        {"name": c.rsplit("/", 1)[1], "path": c} for c in children
                    ],
                    "files": [],
                }
            )
        if path == "/api/v3/tag":
            return self._send(TAGS)
        if path == "/api/v3/movie":
            return self._send(MOVIES)
        if path == "/api/v3/series":
            return self._send(SERIES)
        return self._send({"error": "not found"}, 404)

    def do_PUT(self):
        length = int(self.headers.get("Content-Length", 0))
        json.loads(self.rfile.read(length) or "{}")
        return self._send({"ok": True})

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        self.rfile.read(length)
        return self._send({"id": 1, "status": "queued"})


if __name__ == "__main__":
    HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()

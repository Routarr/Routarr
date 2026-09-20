#!/usr/bin/env python3
"""A TMDb stand-in for the showcase screenshots.

The screenshots exist to show what the rule engine does with real signals, so
the metadata has to be real-shaped: genres, keywords, original language and
certification per title. Anything else would put "no metadata" warnings on
every screen and misrepresent the product.
"""

import json
import os
import re
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(os.environ.get("TMDB_PORT", "7980"))

# tmdb_id -> (genres, language, country, certification, keywords)
MOVIES = {
    149: (["Animation", "Science Fiction", "Action"], "ja", "JP", "R", ["anime", "cyberpunk"]),
    10494: (["Animation", "Thriller", "Mystery"], "ja", "JP", "R", ["anime", "psychological"]),
    129: (["Animation", "Family", "Fantasy"], "ja", "JP", "PG", ["anime"]),
    372058: (["Animation", "Romance", "Drama"], "ja", "JP", "PG", ["anime"]),
    8392: (["Animation", "Family", "Fantasy"], "ja", "JP", "G", ["anime"]),
    862: (["Animation", "Family", "Comedy"], "en", "US", "G", ["toys", "friendship"]),
    116149: (["Family", "Comedy", "Adventure"], "en", "GB", "PG", ["bear", "london"]),
    13260: (["Music", "Documentary"], "en", "US", "NR", ["concert", "live performance", "tour"]),
    1212: (["Music", "Documentary"], "en", "US", "PG", ["concert", "live performance"]),
    603: (["Action", "Science Fiction"], "en", "US", "R", ["dystopia", "artificial intelligence"]),
    335984: (["Science Fiction", "Drama"], "en", "US", "R", ["dystopia", "android"]),
    329865: (["Drama", "Science Fiction", "Mystery"], "en", "US", "PG-13", ["first contact"]),
    438631: (["Science Fiction", "Adventure"], "en", "US", "PG-13", ["desert", "space opera"]),
    244786: (["Drama", "Music"], "en", "US", "R", ["jazz", "drumming"]),
}

SERIES = {
    30991: (["Animation", "Action & Adventure", "Sci-Fi & Fantasy"], "ja", "JP", "TV-14", ["anime", "space western"]),
    31911: (["Animation", "Sci-Fi & Fantasy"], "ja", "JP", "TV-14", ["anime", "alchemy"]),
    68595: (["Documentary"], "en", "GB", "TV-G", ["nature", "wildlife"]),
    62974: (["Documentary"], "en", "US", "TV-14", ["food", "cooking"]),
    63639: (["Sci-Fi & Fantasy", "Drama"], "en", "US", "TV-14", ["space", "politics"]),
    95396: (["Drama", "Mystery"], "en", "US", "TV-MA", ["workplace", "memory"]),
}

FALLBACK = (["Drama"], "en", "US", "NR", [])


def payload(kind, tmdb_id):
    genres, language, country, certification, keywords = (
        MOVIES if kind == "movie" else SERIES
    ).get(tmdb_id, FALLBACK)

    common = {
        "id": tmdb_id,
        "genres": [{"id": i, "name": g} for i, g in enumerate(genres)],
        "original_language": language,
        "origin_country": [country],
        "production_countries": [{"iso_3166_1": country}],
        "overview": "",
        "poster_path": None,
    }

    if kind == "movie":
        common["status"] = "Released"
        common["keywords"] = {"keywords": [{"id": i, "name": k} for i, k in enumerate(keywords)]}
        common["release_dates"] = {
            "results": [{"iso_3166_1": "US", "release_dates": [{"certification": certification}]}]
        }
    else:
        common["status"] = "Ended"
        # TV keywords come back under `results`, not `keywords`.
        common["keywords"] = {"results": [{"id": i, "name": k} for i, k in enumerate(keywords)]}
        common["content_ratings"] = {
            "results": [{"iso_3166_1": "US", "rating": certification}]
        }
    return common


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        path = self.path.split("?")[0]
        match = re.fullmatch(r"/3/(movie|tv)/(\d+)", path)
        if match:
            body = payload(match.group(1), int(match.group(2)))
        elif path == "/3/configuration":
            body = {"images": {}}
        else:
            body = {"status_message": "not found"}

        raw = json.dumps(body).encode()
        self.send_response(200 if match or path == "/3/configuration" else 404)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


if __name__ == "__main__":
    HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()

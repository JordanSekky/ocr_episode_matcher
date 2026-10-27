#!/usr/bin/env python3
"""Collects episode summaries for a season (or every season) into a JSON file, for testing
matching changes.

Each episode gets the TVDB overview the matcher uses (from its cache) and, where the show has a
Fandom wiki, that wiki's synopsis and full plot as plain text.

    python3 eval/fetch_summaries.py --series-id 83237 --season 1 \
        --wiki stargate.fandom.com --category Universe_Season_1_episodes \
        --out eval/stargate-universe/s01-summaries.json

Wiki pages come from the category in its own (airing) order, matched to episode numbers by
position, so check the printed pairing before relying on the output.
"""

import argparse
import html.parser
import json
import pathlib
import re
import sys
import urllib.parse
import urllib.request

CACHE = pathlib.Path.home() / ".config/episode-matcher/cache.json"


class _Text(html.parser.HTMLParser):
    """Paragraph text from a parsed wiki section, without headings, reference markers, tables or
    captions."""

    SKIP = {"table", "sup", "style", "script", "figure", "figcaption", "h2", "h3", "h4"}

    def __init__(self):
        super().__init__()
        self.parts, self.depth = [], 0

    def handle_starttag(self, tag, attrs):
        if tag in self.SKIP:
            self.depth += 1
        elif tag in ("p", "li", "br") and not self.depth:
            self.parts.append("\n")

    def handle_endtag(self, tag):
        if tag in self.SKIP and self.depth:
            self.depth -= 1

    def handle_data(self, data):
        if not self.depth:
            self.parts.append(data)


def plain_text(html_text):
    # Fandom puts a "mark as watched" widget at the top of every section
    html_text = re.sub(
        r'<div id="pt-page-level-tracker".*?</details>\s*</div>', "", html_text, flags=re.S
    )
    parser = _Text()
    parser.feed(html_text)
    text = "".join(parser.parts)
    text = re.sub(r"[ \t]+", " ", text)
    text = re.sub(r"\n\s*\n+", "\n\n", text)
    return text.strip()


def wiki_api(wiki, **params):
    query = urllib.parse.urlencode({**params, "format": "json"})
    request = urllib.request.Request(
        f"https://{wiki}/api.php?{query}", headers={"User-Agent": "episode-matcher-eval/1.0"}
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def wiki_pages(wiki, category):
    data = wiki_api(
        wiki, action="query", list="categorymembers", cmtitle=f"Category:{category}", cmlimit=200
    )
    # The category lists the season overview page too
    return [m["title"] for m in data["query"]["categorymembers"] if m["ns"] == 0
            and not m["title"].lower().startswith("universe season")]


def wiki_sections(wiki, title):
    sections = wiki_api(wiki, action="parse", page=title, prop="sections")["parse"]["sections"]

    def text(index):
        html_text = wiki_api(wiki, action="parse", page=title, prop="text", section=index)
        return plain_text(html_text["parse"]["text"]["*"])

    def section(name):
        return next((text(s["index"]) for s in sections if s["line"] == name), "")

    # Some wikis open every plot with the same recap of the show's premise; it's kept apart since
    # it says nothing about which episode this is
    return section("Synopsis"), section("Prologue"), section("Plot")


def main():
    args = argparse.ArgumentParser()
    args.add_argument("--series-id", required=True)
    args.add_argument("--season", type=int, help="leave out for every season except specials")
    args.add_argument("--wiki", help="Fandom wiki host, e.g. stargate.fandom.com")
    args.add_argument("--category", help="wiki category listing the season's episodes")
    args.add_argument("--out", required=True)
    args = args.parse_args()

    cache = json.loads(CACHE.read_text())
    seasons = cache["episodes_by_sxxexx"][args.series_id]
    wanted = [str(args.season)] if args.season is not None else [s for s in seasons if s != "0"]
    episodes = [
        seasons[s][k] for s in sorted(wanted, key=int) for k in sorted(seasons[s], key=int)
    ]
    show = cache["series"].get(args.series_id)

    titles = wiki_pages(args.wiki, args.category) if args.wiki else []
    if titles and len(titles) != len(episodes):
        sys.exit(f"{len(titles)} wiki pages for {len(episodes)} episodes; pair them by hand")

    out = []
    for i, ep in enumerate(episodes):
        name = " ".join(ep["name"].split())
        entry = {
            "label": f"S{ep['season_number']:02}E{ep['episode_number']:02} - {name}",
            "season": ep["season_number"],
            "episode": ep["episode_number"],
            "name": name,
            "tvdb_overview": (ep.get("overview") or "").strip(),
        }
        if titles:
            synopsis, prologue, plot = wiki_sections(args.wiki, titles[i])
            entry.update(
                wiki_title=titles[i],
                wiki_url=f"https://{args.wiki}/wiki/{urllib.parse.quote(titles[i].replace(' ', '_'))}",
                wiki_synopsis=synopsis,
                wiki_prologue=prologue,
                wiki_plot=plot,
            )
            print(f"{entry['label']}  <-  {titles[i]}  ({len(plot)} chars of plot)")
        out.append(entry)

    document = {
        "show": show,
        "tvdb_series_id": args.series_id,
        "season": args.season,
        "sources": {
            "tvdb_overview": "TheTVDB, from the episode-matcher cache",
            "wiki": f"https://{args.wiki} (Fandom, text under CC BY-SA)" if args.wiki else None,
        },
        "episodes": out,
    }
    path = pathlib.Path(args.out)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n")
    print(f"Wrote {path}")


if __name__ == "__main__":
    main()

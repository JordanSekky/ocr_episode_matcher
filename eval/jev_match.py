#!/usr/bin/env python3
"""Scores TypeSafe's Jev classifier on an eval set, as a comparison for the matcher's LLM.

    python3 eval/jev_match.py --summaries eval/stargate-universe/s01-summaries.json \
        --eval-set eval/stargate-universe/eval-set.tsv --subtitles DIR \
        --source tvdb_overview --mode choice --out results.json

--subtitles holds one <id>.txt per eval file with its subtitle text. --source picks which summary
from the summaries file describes each episode. --mode choice asks one multiple-choice question
over every episode; --mode yesno asks "is it this episode?" once per episode and takes the most
likely, which is the only way long summaries fit Jev's request limits.
"""

import argparse
import json
import pathlib
import sys
import time
import urllib.error
import urllib.request

API = "https://api.typesafe.ai/v1/systemone"
NO_MATCH = "NO MATCH"
# Jev's limits as reported in third-party docs: 64K tokens a request, of which the state plus the
# longest question may use 32K. Estimated at 4 characters a token, with headroom.
REQUEST_CHARS = 200_000
QUESTION_CHARS = 100_000
# Per million input tokens; output tokens are free. Used when the response has no cost header.
INPUT_PRICE = 0.042

CHOICE_INSTRUCTIONS = (
    "Which TV episode is this subtitle excerpt from? The excerpt is OCR'd dialogue and may come "
    "from any point within the episode. Each option is an episode, described by its plot; pick "
    f"{NO_MATCH} if it is none of them, such as an extra, trailer or menu."
)


def yesno_instructions(episode, text):
    return (
        "Is this subtitle excerpt from the TV episode described below? The excerpt is OCR'd "
        "dialogue and may come from any point within the episode.\n\n"
        f"Episode: {episode['label']}\n\n{text}"
    )


def ask(key, model, state, questions):
    body = json.dumps({"model": model, "state": state, "questions": questions}).encode()
    request = urllib.request.Request(
        API,
        data=body,
        headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            header = response.headers.get("X-Cost-USD")
            data = json.load(response)
            tokens = data.get("usage", {}).get("input_tokens", 0)
            cost = float(header) if header else tokens * INPUT_PRICE / 1e6
            return data, cost
    except urllib.error.HTTPError as e:
        sys.exit(f"Jev returned HTTP {e.code}: {e.read().decode(errors='replace')[:2000]}")


def chunks(questions, state_chars):
    """Splits {name: question} into requests that fit Jev's limits."""
    batch, size = {}, state_chars
    for name, question in questions.items():
        length = len(json.dumps(question))
        if state_chars + length > QUESTION_CHARS:
            sys.exit(f"question {name} is too long for Jev ({length} chars)")
        if batch and size + length > REQUEST_CHARS:
            yield batch
            batch, size = {}, state_chars
        batch[name] = question
        size += length
    if batch:
        yield batch


def main():
    args = argparse.ArgumentParser()
    args.add_argument("--summaries", required=True)
    args.add_argument("--eval-set", required=True, help="TSV of id, expected SxxExx, path")
    args.add_argument("--subtitles", required=True)
    args.add_argument("--source", default="tvdb_overview",
                      help="summary field: tvdb_overview, tmdb_overview, wiki_synopsis or wiki_plot")
    args.add_argument("--mode", choices=("choice", "yesno"), default="choice")
    args.add_argument("--model", default="jev-latest")
    args.add_argument("--key-file", default="jev-key.txt")
    args.add_argument("--per-season", action="store_true",
                      help="only offer episodes from the expected answer's season, as the "
                           "matcher's --season does")
    args.add_argument("--out", required=True)
    args = args.parse_args()

    key = pathlib.Path(args.key_file).read_text().strip()
    summaries = json.loads(pathlib.Path(args.summaries).read_text())
    episodes = summaries["episodes"]
    by_code = {e["label"][:6]: e for e in episodes}
    expected = {
        line.split("\t")[0]: line.split("\t")[1]
        for line in pathlib.Path(args.eval_set).read_text().splitlines()
        if line.strip()
    }

    results, correct, total_cost = [], 0, 0.0
    for file_id, want in expected.items():
        subtitles = (pathlib.Path(args.subtitles) / f"{file_id}.txt").read_text()
        offered = [e for e in episodes if not args.per_season or e["label"][:3] == want[:3]]
        state = f"Show: {summaries['show']}\n\nSubtitle excerpt:\n{subtitles}"
        start = time.time()

        if args.mode == "choice":
            criteria = {e["label"]: e[args.source] or None for e in offered}
            criteria[NO_MATCH] = "None of these episodes."
            questions = {"episode": {"type": "choice", "instructions": CHOICE_INSTRUCTIONS,
                                     "criteria": criteria}}
            response, cost = ask(key, args.model, state, questions)
            answer = response["answers"]["episode"]
            got = answer["choice"]
            score = answer.get("confidence")
            raw = [response]
        else:
            questions = {
                e["label"][:6]: {"type": "noul",
                                 "instructions": yesno_instructions(e, e[args.source])}
                for e in offered
            }
            probabilities, raw, cost = {}, [], 0.0
            for batch in chunks(questions, len(state)):
                response, batch_cost = ask(key, args.model, state, batch)
                cost += batch_cost
                raw.append(response)
                for code, answer in response["answers"].items():
                    probabilities[code] = answer["noul"]
            code = max(probabilities, key=probabilities.get)
            got = by_code[code]["label"]
            score = probabilities[code]

        ok = got[:6] == want
        correct += ok
        total_cost += cost
        print(f"{file_id} want {want} got {got} ({score}) {'OK' if ok else 'XX'} "
              f"{time.time() - start:.1f}s ${cost:.5f}")
        results.append({"id": file_id, "expected": want, "got": got, "score": score,
                        "correct": ok, "cost_usd": cost, "responses": raw})

    print(f"score {correct}/{len(expected)}, cost ${total_cost:.4f}")
    pathlib.Path(args.out).write_text(json.dumps(
        {"source": args.source, "mode": args.mode, "model": args.model,
         "per_season": args.per_season,
         "score": correct, "of": len(expected), "results": results}, indent=2))


if __name__ == "__main__":
    main()

"""Scores bench JSON results: WER, term recall and latency per engine and set. Usage: score.py fixture/utterances.tsv results/*.json"""
import csv, json, re, statistics, sys

NUM = {"0": "zero", "1": "one", "2": "two", "3": "three", "4": "four", "5": "five", "6": "six", "7": "seven",
       "8": "eight", "9": "nine", "10": "ten", "12": "twelve", "212": "two twelve", "7422": "seven four two two"}


def norm(s):
    s = s.lower().replace("-", " ").replace(".", " ").replace("/", " ")
    s = re.sub(r"[^a-z0-9' ]", " ", s)
    s = s.replace("'", "")
    words = []
    for w in s.split():
        words.extend(NUM.get(w, w).split())
    return " ".join(words)


def edits(r, h):
    d = list(range(len(h) + 1))
    for i in range(1, len(r) + 1):
        prev, d[0] = d[0], i
        for j in range(1, len(h) + 1):
            cur = min(d[j] + 1, d[j - 1] + 1, prev + (r[i - 1] != h[j - 1]))
            prev, d[j] = d[j], cur
    return d[len(h)]


def main():
    ref = {}
    with open(sys.argv[1]) as f:
        for row in csv.DictReader(f, delimiter="\t"):
            ref[row["id"]] = (norm(row["text"]), [norm(t) for t in row["terms"].split("|")])
    rows = []
    for path in sys.argv[2:]:
        r = json.load(open(path))
        for st in ("clean", "noisy"):
            cs = [c for c in r["clips"] if c["set"] == st]
            e = n = hit = tn = 0
            for c in cs:
                rw, terms = ref[c["id"]]
                hw = norm(c["text"])
                e += edits(rw.split(), hw.split()); n += len(rw.split())
                hit += sum(1 for t in terms if f" {t} " in f" {hw} "); tn += len(terms)
            lat = sorted(c["latency_s"] for c in cs)
            rows.append((r["engine"], r.get("platform", "Mac" if "/mac" in path else "iPhone"), st, 100 * e / n, 100 * hit / tn,
                         statistics.median(lat) * 1000, lat[int(0.9 * (len(lat) - 1))] * 1000, r["load_s"], r["peak_mb"]))
    print("| Engine | Platform | Set | WER % | Terms % | Median ms | p90 ms | Load s | Peak MB |")
    print("|---|---|---|---|---|---|---|---|---|")
    for x in rows:
        print(f"| {x[0]} | {x[1]} | {x[2]} | {x[3]:.1f} | {x[4]:.0f} | {x[5]:.0f} | {x[6]:.0f} | {x[7]:.1f} | {x[8]:.0f} |")


if __name__ == "__main__":
    assert edits("a b c".split(), "a x c".split()) == 1 and norm("wheel-2 PR #212.") == "wheel two pr two twelve"
    main()

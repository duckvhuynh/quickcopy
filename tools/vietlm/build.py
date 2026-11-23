"""Builds the Vietnamese syllable table QuickCopy uses to pick tone marks
(src/vietlm.rs) from Vietnamese Wikipedia.

    python build.py <out.bin> <wiki parquet>...

Input: the `wikimedia/wikipedia` dataset's 20231101.vi parquet files
(https://huggingface.co/datasets/wikimedia/wikipedia, text CC BY-SA 4.0 /
GFDL). Needs pyarrow and numpy.

The text is lowercased and cut into syllables: runs of letters from
VieSVTR's alphabet. Anything else (digits, punctuation) ends a run of
neighbours, so a bigram is two syllables with only a space between them,
as on a line QuickCopy reads.

Output, little-endian:
  b"VLM1", u32 vocabulary size N, u32 byte length, the N syllables joined
  by "\\n" (most frequent first), N bytes of unigram cost, then for each
  syllable in order a varint count of the syllables seen after it,
  and per follower a varint id delta (ascending) and a byte of cost.
A cost is -ln(probability) in steps of 0.1, at most 255.
"""
import math
import os
import re
import sys
from collections import Counter
from multiprocessing import Pool

import numpy as np
import pyarrow.parquet as pq

HERE = os.path.dirname(os.path.abspath(__file__))
VOCAB = open(os.path.join(HERE, "..", "..", "src", "vocab.txt"), encoding="utf-8").read()
LETTERS = "".join(sorted({c.lower() for c in VOCAB if c.isalpha()}))
TOKEN = re.compile(f"[{LETTERS}]+|\\S")
MIN_COUNT = 20  # a syllable seen fewer times is left out
MIN_PAIR = 3  # likewise for a pair
MAX_VOCAB = 65535


def texts(path):
    for batch in pq.ParquetFile(path).iter_batches(batch_size=2000, columns=["text"]):
        for t in batch.column(0).to_pylist():
            yield t.lower()


def is_word(tok):
    return tok[0] in LETTERS


def unigrams(path):
    c = Counter()
    for t in texts(path):
        c.update(w for w in TOKEN.findall(t) if is_word(w))
    return c


def bigrams(args):
    path, ids = args
    keys, counts = np.zeros(0, np.int64), np.zeros(0, np.int64)
    pending = []

    def merge(keys, counts, chunk):
        k, c = np.unique(np.concatenate(chunk), return_counts=True)
        allk = np.concatenate([keys, k])
        allc = np.concatenate([counts, c])
        order = np.argsort(allk, kind="stable")
        allk, allc = allk[order], allc[order]
        starts = np.flatnonzero(np.r_[True, allk[1:] != allk[:-1]])
        return allk[starts], np.add.reduceat(allc, starts)

    size = 0
    for t in texts(path):
        a = np.fromiter((ids.get(w, -1) for w in TOKEN.findall(t)), np.int64)
        if len(a) < 2:
            continue
        ok = (a[:-1] >= 0) & (a[1:] >= 0)
        k = a[:-1][ok] * 65536 + a[1:][ok]
        pending.append(k)
        size += len(k)
        if size > 30_000_000:
            keys, counts = merge(keys, counts, pending)
            pending, size = [], 0
    if pending:
        keys, counts = merge(keys, counts, pending)
    return keys, counts


def varint(n, out):
    while n >= 0x80:
        out.append((n & 0x7F) | 0x80)
        n >>= 7
    out.append(n)


def cost(p):
    return min(255, round(-math.log(p) / 0.1))


def main():
    out_path, paths = sys.argv[1], sys.argv[2:]
    cache = os.environ.get("VLM_CACHE")  # counts saved here, to try other limits without recounting
    if cache and os.path.exists(cache):
        z = np.load(cache, allow_pickle=True)
        uni, keys, counts = Counter(z["uni"].item()), z["keys"], z["counts"]
    else:
        uni, keys, counts = count(paths)
        if cache:
            np.savez(cache, uni=np.array(dict(uni), dtype=object), keys=keys, counts=counts)
    words = [w for w, n in uni.most_common(MAX_VOCAB) if n >= MIN_COUNT]
    total = sum(uni.values())
    print(f"{total} syllables, {len(uni)} distinct, {len(words)} kept", flush=True)
    # Pairs were counted for the syllables kept at MIN_COUNT; ids are ranks.
    keep = (counts >= MIN_PAIR) & (keys // 65536 < len(words)) & (keys % 65536 < len(words))
    keys, counts = keys[keep], counts[keep]
    print(f"{len(keys)} pairs kept", flush=True)
    write(out_path, words, uni, total, keys, counts)


def count(paths):
    with Pool(len(paths)) as pool:
        uni = Counter()
        for c in pool.map(unigrams, paths):
            uni.update(c)
        words = [w for w, n in uni.most_common(MAX_VOCAB) if n >= MIN_COUNT]
        ids = {w: i for i, w in enumerate(words)}
        keys, counts = np.zeros(0, np.int64), np.zeros(0, np.int64)
        for k, c in pool.map(bigrams, [(p, ids) for p in paths]):
            allk, allc = np.concatenate([keys, k]), np.concatenate([counts, c])
            order = np.argsort(allk, kind="stable")
            allk, allc = allk[order], allc[order]
            starts = np.flatnonzero(np.r_[True, allk[1:] != allk[:-1]])
            keys, counts = allk[starts], np.add.reduceat(allc, starts)
    return uni, keys, counts


def write(out_path, words, uni, total, keys, counts):
    out = bytearray(b"VLM1")
    joined = "\n".join(words).encode("utf-8")
    out += len(words).to_bytes(4, "little") + len(joined).to_bytes(4, "little") + joined
    out += bytes(cost(uni[w] / total) for w in words)
    first, second = keys // 65536, keys % 65536
    bounds = np.searchsorted(first, np.arange(len(words) + 1))
    for i, w in enumerate(words):
        lo, hi = bounds[i], bounds[i + 1]
        varint(int(hi - lo), out)
        prev = 0
        for j in range(lo, hi):
            varint(int(second[j]) - prev, out)
            prev = int(second[j])
            out.append(cost(int(counts[j]) / uni[w]))
    open(out_path, "wb").write(out)
    print(f"wrote {out_path}: {len(out)} bytes", flush=True)


if __name__ == "__main__":
    main()

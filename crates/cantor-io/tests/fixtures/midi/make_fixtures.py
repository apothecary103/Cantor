#!/usr/bin/env python3
"""Writes the MIDI importer test fixtures (raw SMF bytes).

Run from anywhere: python3 make_fixtures.py
Standard library only; the output is deterministic.
"""

import os
import struct

HERE = os.path.dirname(os.path.abspath(__file__))


def vlq(n):
    out = [n & 0x7F]
    n >>= 7
    while n:
        out.append(0x80 | (n & 0x7F))
        n >>= 7
    return bytes(reversed(out))


def meta(kind, data):
    return bytes([0xFF, kind]) + vlq(len(data)) + data


def text(s):
    return meta(0x01, s.encode("utf-8"))


def name(s):
    return meta(0x03, s.encode("utf-8"))


def lyric(s):
    data = s if isinstance(s, bytes) else s.encode("utf-8")
    return meta(0x05, data)


def marker(s):
    return meta(0x06, s.encode("utf-8"))


def tempo(us):
    return meta(0x51, us.to_bytes(3, "big"))


def time_sig(num, den_pow):
    return meta(0x58, bytes([num, den_pow, 24, 8]))


def key_sig(sf, minor):
    return meta(0x59, struct.pack("bB", sf, 1 if minor else 0))


def on(ch, key, vel=100):
    return bytes([0x90 | ch, key, vel])


def off(ch, key):
    return bytes([0x80 | ch, key, 64])


def cc(ch, num, val):
    return bytes([0xB0 | ch, num, val])


def bend(ch, value):
    return bytes([0xE0 | ch, value & 0x7F, value >> 7])


def program(ch, prog):
    return bytes([0xC0 | ch, prog])


def track(events):
    """events: list of (absolute tick, bytes); sorted stably by tick."""
    events = sorted(enumerate(events), key=lambda e: (e[1][0], e[0]))
    body = b""
    now = 0
    for _, (tick, data) in events:
        body += vlq(tick - now) + data
        now = tick
    body += vlq(0) + meta(0x2F, b"")
    return b"MTrk" + struct.pack(">I", len(body)) + body


def smf(fmt, ppq, tracks):
    head = b"MThd" + struct.pack(">IHHH", 6, fmt, len(tracks), ppq)
    return head + b"".join(tracks)


def basic():
    q = 480  # PPQ
    bar4 = 4 * q
    bar3 = 3 * q
    conductor = track(
        [
            (0, name("Basic Song")),
            (0, tempo(500_000)),  # 120 BPM
            (0, time_sig(4, 2)),  # 4/4
            (0, key_sig(1, False)),  # G major
            (2 * bar4, tempo(600_000)),  # 100 BPM at bar 3
            (2 * bar4, time_sig(3, 2)),  # 3/4 at bar 3
            (2 * bar4, key_sig(1, True)),  # E minor at bar 3
            (2 * bar4, marker("Verse")),
            # Half a beat into bar 6 (0-based 5): moved to bar 7 (0-based 6).
            (2 * bar4 + 3 * bar3 + q // 2, time_sig(4, 2)),
        ]
    )

    # Ten quarter notes: C D E F G | A B C' (bar 2) ... with lyrics.
    keys = [60, 62, 64, 65, 67, 69, 71, 72, 74, 76]
    words = ["Twin-", "kle", "twin-", "kle", "lit-", "tle", None, "ら", b"\xffla", "star"]
    melody = [(0, name("Melody")), (0, program(0, 52))]
    for i, (key, word) in enumerate(zip(keys, words)):
        start = i * q
        melody.append((start, on(0, key)))
        if word is not None:
            melody.append((start, lyric(word)))
        # The fourth note ends with a NoteOn of velocity 0.
        melody.append((start + q, on(0, key, 0) if i == 3 else off(0, key)))
    melody.append((10 * q + q // 2, lyric("orphan")))  # no note here

    harmony = track(
        [
            (0, name("Harmony")),
            (0, text("ooh")),
            (0, on(1, 55)),
            (0, cc(1, 7, 100)),
            (q // 2, bend(1, 9000)),
            (q, on(1, 57)),  # overlaps the first note
            (2 * q, off(1, 55)),
            (3 * q, off(1, 57)),
            # Two notes starting together: the lower one is dropped.
            (4 * q, on(1, 60)),
            (4 * q, on(1, 64)),
            (4 * q, text("aah")),
            (5 * q, off(1, 60)),
            (5 * q, off(1, 64)),
            # A zero-length note and an unmatched note-off.
            (6 * q, on(1, 62)),
            (6 * q, off(1, 62)),
            (6 * q, off(1, 70)),
            # A drum hit in a melodic track.
            (6 * q, on(9, 38)),
            (6 * q + 10, off(9, 38)),
            # Never released: ends at the end of the track.
            (7 * q, on(1, 59)),
            (9 * q, cc(1, 64, 0)),
        ]
    )

    drums = track(
        [
            (0, name("Drums")),
            (0, on(9, 36)),
            (q, off(9, 36)),
        ]
    )
    return smf(1, q, [conductor, track(melody), harmony, drums])


def odd_ppq():
    q = 1000  # does not divide 1920
    events = [
        (0, name("Odd")),
        (0, time_sig(6, 3)),  # 6/8
        (q, tempo(483_871)),  # ~124.0 BPM, and no tempo at tick 0
    ]
    starts = [0, 333, 667, 1000, 1500, 2001, 2999]
    words = ["do", "re", "mi", "fa", "so", "la", "ti"]
    for i, (start, word) in enumerate(zip(starts, words)):
        end = starts[i + 1] if i + 1 < len(starts) else start + 1001
        events.append((start, on(0, 60 + 2 * i)))
        events.append((start, lyric(word)))
        events.append((end, off(0, 60 + 2 * i)))
    return smf(0, q, [track(events)])


def main():
    for file, data in [("basic.mid", basic()), ("odd_ppq.mid", odd_ppq())]:
        with open(os.path.join(HERE, file), "wb") as f:
            f.write(data)


if __name__ == "__main__":
    main()

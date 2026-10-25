"""A reference svs.io backend for Cantor's adapter tests.

Speaks Cantor's interpretation of OpenUtau's svs.io proposal
(docs/decisions/0023-svsio.md): ZeroMQ REP, one msgpack map per message.
Ops:
  phonemize            note_sequence -> phoneme_sequence
  synth_world_features phoneme_sequence + f0 (+ curve:breathiness)
                       -> world_sp + world_ap (WORLD power spectrum and
                       aperiodicity, 1025 bins, 5 ms frames, 44.1 kHz)
  synth_audio_samples  phoneme_sequence + f0 -> audio_samples (int16)
The voice is a crude source-filter model: vowels a i u e o as formant
envelopes, consonants as noise. Pure Python (pyzmq, msgpack), deterministic.

Usage: python3 reference_backend.py [--port N]   (prints the port it bound)
"""

import math
import sys

import msgpack
import zmq

SR = 44100
FFT = 2048
BINS = FFT // 2 + 1
FRAME_MS = 5.0
FORMANTS = {
    "a": (800, 1250, 2600),
    "i": (300, 2250, 3000),
    "u": (350, 1250, 2350),
    "e": (480, 1850, 2550),
    "o": (480, 850, 2600),
}
CONSONANT_MS = 60.0


def describe():
    frames = {"time_unit": "ms", "frame_duration": FRAME_MS}
    return {
        "name": "cantor-reference",
        "version": "1",
        "ops": [
            {
                "op": "phonemize",
                "inputs": {"note_sequence": {"required": True}},
                "outputs": {"phoneme_sequence": {"time_unit": "ms"}},
            },
            {
                "op": "synth_world_features",
                "inputs": {
                    "phoneme_sequence": {"required": True},
                    "f0": dict(frames, required=True),
                    "curve:breathiness": dict(frames, required=False, scale="db"),
                },
                "outputs": {
                    "world_sp": dict(frames, width=BINS),
                    "world_ap": dict(frames, width=BINS),
                },
            },
            {
                "op": "synth_audio_samples",
                "inputs": {
                    "phoneme_sequence": {"required": True},
                    "f0": dict(frames, required=True),
                },
                "outputs": {
                    "audio_samples": {
                        "channels": 1,
                        "sample_rate": SR,
                        "sample_format": "int16",
                    }
                },
            },
        ],
    }


def phonemize(ns):
    """Splits each lyric into an optional consonant and a vowel; the
    consonant borrows CONSONANT_MS from what comes before the note."""
    out = []
    for i, n in enumerate(ns["notes"]):
        lyric, dur, key = n["lyric"], float(n["duration"]), n["key"]
        vowel = lyric[-1:] if lyric[-1:] in FORMANTS else ""
        cons = lyric[:-1] if vowel else ""
        if lyric and not vowel:
            out.append({"note_index": i, "phoneme": "", "duration": dur, "key": key})
            continue
        if cons and out and out[-1]["duration"] > 2 * CONSONANT_MS:
            out[-1]["duration"] -= CONSONANT_MS
            out.append({"note_index": i, "phoneme": cons, "duration": CONSONANT_MS, "key": key})
        out.append({"note_index": i, "phoneme": vowel, "duration": dur, "key": key})
    return {"time_unit": "ms", "phonemes": out}


def frame_phonemes(seq, frames):
    """The phoneme sounding in each 5 ms frame."""
    labels, t = [], 0.0
    bounds = []
    for p in seq["phonemes"]:
        bounds.append((t, t + float(p["duration"]), p["phoneme"]))
        t += float(p["duration"])
    j = 0
    for f in range(frames):
        ms = f * FRAME_MS
        while j + 1 < len(bounds) and ms >= bounds[j][1]:
            j += 1
        labels.append(bounds[j][2] if bounds else "")
    return labels


def envelope(phoneme):
    """(sp, ap) rows for one phoneme."""
    if phoneme in FORMANTS:
        f1, f2, f3 = FORMANTS[phoneme]
        sp = []
        for b in range(BINS):
            hz = b * SR / FFT
            g = 0.02
            for f, bw, amp in ((f1, 90, 1.0), (f2, 110, 0.5), (f3, 150, 0.25)):
                g += amp * math.exp(-(((hz - f) / bw) ** 2))
            sp.append(1e-5 * g * g / (1.0 + (hz / 3000.0) ** 2))
        return sp, [0.02] * BINS
    if phoneme:
        return [1e-7 * (1.0 + (b / BINS)) for b in range(BINS)], [0.95] * BINS
    return [1e-14] * BINS, [0.999] * BINS


def synth_world(req):
    seq, f0 = req["phoneme_sequence"], req["f0"]
    frames = len(f0["f0"])
    breath = req.get("curve:breathiness", {}).get("curve", [])
    cache, sp_rows, ap_rows = {}, [], []
    for f, ph in enumerate(frame_phonemes(seq, frames)):
        if ph not in cache:
            cache[ph] = envelope(ph)
        sp, ap = cache[ph]
        b = breath[f] if f < len(breath) else 0.0
        if b:
            k = 10 ** (b / 20.0)
            ap = [min(0.999, a * k) for a in ap]
        sp_rows.append(sp)
        ap_rows.append(ap)
    meta = {"time_unit": "ms", "frame_duration": FRAME_MS, "width": BINS}
    return {
        "world_sp": dict(meta, world_sp=sp_rows),
        "world_ap": dict(meta, world_ap=ap_rows),
    }


def synth_audio(req):
    seq, f0 = req["phoneme_sequence"], req["f0"]
    cents = f0["f0"]
    frames = len(cents)
    labels = frame_phonemes(seq, frames)
    hop = SR * FRAME_MS / 1000.0
    n = int(frames * hop)
    out, phase, noise = [], 0.0, 12345
    for i in range(n):
        f = min(int(i / hop), frames - 1)
        hz = 440.0 * 2 ** ((cents[f] / 100.0 - 69.0) / 12.0)
        phase += 2 * math.pi * hz / SR
        ph = labels[f]
        if ph in FORMANTS:
            f1, f2, _ = FORMANTS[ph]
            s = 0.0
            for k in range(1, 9):
                h = k * hz
                w = math.exp(-(((h - f1) / 300) ** 2)) + 0.5 * math.exp(-(((h - f2) / 400) ** 2)) + 0.05
                s += w * math.sin(k * phase) / k
            v = 0.3 * s
        elif ph:
            noise = (noise * 1103515245 + 12345) % 2147483648
            v = 0.05 * (noise / 1073741824.0 - 1.0)
        else:
            v = 0.0
        out.append(max(-32768, min(32767, int(round(v * 32767)))))
    return {
        "audio_samples": {
            "channels": 1,
            "sample_rate": SR,
            "sample_format": "int16",
            "samples": [out],
        }
    }


def handle(req):
    op = req.get("op")
    if op == "ops":
        return describe()
    if op == "phonemize":
        return {"phoneme_sequence": phonemize(req["note_sequence"])}
    if op == "synth_world_features":
        return synth_world(req)
    if op == "synth_audio_samples":
        return synth_audio(req)
    return {"error": f"unknown op {op!r}"}


def main():
    port = 0
    if "--port" in sys.argv:
        port = int(sys.argv[sys.argv.index("--port") + 1])
    ctx = zmq.Context()
    sock = ctx.socket(zmq.REP)
    if port:
        sock.bind(f"tcp://127.0.0.1:{port}")
    else:
        port = sock.bind_to_random_port("tcp://127.0.0.1")
    print(port, flush=True)
    while True:
        req = msgpack.unpackb(sock.recv(), raw=False)
        try:
            reply = handle(req)
        except Exception as e:  # report, do not die
            reply = {"error": f"{type(e).__name__}: {e}"}
        sock.send(msgpack.packb(reply, use_bin_type=True))


if __name__ == "__main__":
    main()

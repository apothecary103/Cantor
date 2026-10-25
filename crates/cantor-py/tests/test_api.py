"""API checks for the cantor package, run against an installed wheel."""
import math
import sys

import cantor as ct

p = ct.open(sys.argv[1])
assert p.revision == 0
lead = p.track("LEAD")
assert lead.name == "Lead"
notes = lead.notes()
assert len(notes) == 8 and notes[0].lyric == "yi"
assert [n.id for n in lead.notes(section="chorus")] == [104, 105, 106, 107]
assert [n.id for n in notes if n.is_phrase_end] == [103, 107]

# Edits outside a transaction apply immediately; inside, as one batch.
notes[0].lyric = "la"
assert p.revision == 1 and notes[0].lyric == "la"
with p.transaction("two edits"):
    notes[1].lyric = "ma"
    notes[1].vibrato(depth_cents=40, rate_hz=5.0)
    assert p.revision == 1, "deferred until the block ends"
assert p.revision == 2 and notes[1].lyric == "ma"

# A failing block applies nothing.
try:
    with p.transaction("aborted"):
        notes[2].lyric = "x"
        raise RuntimeError("stop")
except RuntimeError:
    pass
assert p.revision == 2 and notes[2].lyric == "yi"

# Invalid edits raise and change nothing.
try:
    notes[3].curve("breathiness").ramp(to=1e9)
    raise AssertionError("expected ValueError")
except ValueError as e:
    assert "curve.value_out_of_range" in str(e)
try:
    lead.notes(section="bridge")
    raise AssertionError("expected CantorError")
except ct.CantorError:
    pass

# Style prompts become curve and expression edits in one step.
rev = p.revision
assert lead.style("breathier and more intimate, slightly less vibrato", 0, 7680) == []
assert p.revision == rev + 1
assert lead.style("sparkly", 0, 7680) == ["sparkly"]

# Effects and doubling are track settings with registry-checked parameters.
lead.set_effects([
    {"kind": "eq", "params": {"presence_gain_db": 2.0}},
    {"kind": "reverb", "params": {"mix": 0.15}},
])
lead.set_doubling(layers=2, spacing=0.6)
try:
    lead.set_effects([{"kind": "reverb", "params": {"wetness": 1.0}}])
    raise AssertionError("expected ValueError")
except ValueError as e:
    assert "wetness" in str(e)

report = p.render(quality="preview", start=0, end=7680)
metrics = {m.id: m for m in report.notes}
assert not math.isnan(metrics[100].pitch_error_p95)
assert 104 not in metrics, "phrases outside the range are not rendered or reported"
assert report.audio_path.endswith(".wav") and len(report.audio_hash) == 64
assert "LUFS" in report.summary()
print("test_api: ok")

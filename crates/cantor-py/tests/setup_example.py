"""Prepares song.cantor for the docs/spec/60-api.md example: the fixture
song with a "chorus" section over its second phrase."""
import json
import sys

import cantor as ct

p = ct.open(sys.argv[1])
p.apply_json(json.dumps({"op": "set_section", "name": "chorus", "start": 9600, "end": 17280}))
p.save()
print("prepared", p)

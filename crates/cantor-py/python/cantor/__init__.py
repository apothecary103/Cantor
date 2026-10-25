"""Cantor: scripting for the Cantor singing voice synthesis workstation.

Every edit is a Cantor operation applied through the same engine the GUI,
CLI and MCP server use. See docs/spec/60-api.md.
"""

from ._native import (
    CantorError,
    Curve,
    Note,
    NoteMetric,
    Project,
    Report,
    Track,
    Transaction,
    convert_audio,
    import_file,
    new,
    open,
)

__all__ = [
    "CantorError",
    "Curve",
    "Note",
    "NoteMetric",
    "Project",
    "Report",
    "Track",
    "Transaction",
    "convert_audio",
    "import_file",
    "new",
    "open",
]

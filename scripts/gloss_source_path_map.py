#!/usr/bin/env python3
"""Canonical source paths for current Gloss validation scripts."""
from __future__ import annotations

CURRENT_SOURCE_PATHS = {
    "chat_commands": "src-tauri/src/commands/chat/mod.rs",
    "source_commands": "src-tauri/src/commands/sources/mod.rs",
    "notebook_db": "src-tauri/src/db/notebook_db/mod.rs",
    "settings_dialog": "src/components/settings/SettingsDialog/index.tsx",
}


def stale_source_paths() -> list[str]:
    return [
        "src-tauri/src/commands/" + "chat.rs",
        "src-tauri/src/commands/" + "sources.rs",
        "src-tauri/src/db/" + "notebook_db.rs",
        "src/components/settings/" + "SettingsDialog.tsx",
    ]

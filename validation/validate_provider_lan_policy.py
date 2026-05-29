#!/usr/bin/env python3
"""Static gate for explicit local-provider LAN policy."""
from pathlib import Path
import sys
root = Path(sys.argv[1]) if len(sys.argv)>1 else Path('.')
text = (root/'src-tauri/src/providers/mod.rs').read_text(encoding='utf-8')
if 'allow_lan_local_providers' not in text and 'LanLocalProvider' not in text and 'local_provider_network_scope' not in text:
    print('WARN: no explicit LAN local-provider opt-in found. This is acceptable only if live trace does not require LAN Ollama.')
    sys.exit(0)
required = ['192.168', '10.', '172.16', 'public', 'credentials', 'query']
missing = [x for x in required if x not in text]
if missing:
    print('FAIL: LAN policy exists but tests/guards appear incomplete, missing markers:', missing)
    sys.exit(1)
print('PASS: explicit LAN provider policy markers present')

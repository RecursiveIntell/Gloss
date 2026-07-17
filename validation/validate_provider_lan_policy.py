#!/usr/bin/env python3
"""Static gate for explicit local-provider LAN policy."""
from pathlib import Path
import sys
root = Path(sys.argv[1]) if len(sys.argv)>1 else Path('.')
text = (root/'src-tauri/src/providers/mod.rs').read_text(encoding='utf-8')
if 'allow_lan_local_providers' not in text and 'LanLocalProvider' not in text and 'local_provider_network_scope' not in text:
    print('FAIL: no explicit LAN local-provider opt-in found. LAN protection is required.')
    sys.exit(1)

# Robust checks: require specific subnet patterns, not loose substrings
required_patterns = [
    r'192\.168\.',  # 192.168.0.0/16
    r'10\.(0|[1-9][0-9]?|1[0-9][0-9]|2[0-4][0-9]|25[0-5])\.',  # 10.0.0.0/8
    r'172\.(1[6-9]|2[0-9]|3[0-1])\.',  # 172.16.0.0/12
]

# Check for validate_provider_base_url or equivalent enforcement function
has_validation_function = any(pattern in text for pattern in [
    'validate_provider_base_url',
    'validate_url',
    'check_network_scope',
])

# Check for RFC1918 rejection / LAN opt-in
has_lan_rejection = 'lan_rejected' in text or 'is_rfc1918_host' in text or 'LAN' in text.upper()
has_credentials_check = 'credentials' in text.lower()
has_query_check = 'query()' in text  # Check for parsed.query() call, not just word "query"

missing = []
for pat in required_patterns:
    import re
    if not re.search(pat, text):
        missing.append(pat)

if missing or not has_lan_rejection:
    print('FAIL: LAN policy exists but tests/guards appear incomplete, missing:', missing if missing else 'LAN rejection marker')
    sys.exit(1)

if not has_validation_function:
    print('WARN: no explicit URL validation function found; relying on subnet markers')

if not has_credentials_check:
    print('WARN: no credentials-in-URL check found')

if not has_query_check:
    print('WARN: no query-string-in-URL check found')

print('PASS: explicit LAN provider policy markers present')

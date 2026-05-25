#!/usr/bin/env python3
import argparse, json
from pathlib import Path
REQUIRED=['PDF ingestion','DOCX ingestion','XLSX ingestion','URL import','YouTube transcript import','Audio transcription','Audio overview/TTS','Studio reports','Studio flashcards/quizzes','Notebook export/import','Desktop smoke','DB doctor']
def main():
 ap=argparse.ArgumentParser(); ap.add_argument('--repo', default='.') ; args=ap.parse_args(); root=Path(args.repo)
 paths=[root/'docs/CURRENT_FEATURE_MATRIX.md', root/'CURRENT_FEATURE_MATRIX.md']
 txt='\n'.join(p.read_text(errors='ignore') for p in paths if p.exists())
 failures=[]
 if not txt.strip(): failures.append('missing CURRENT_FEATURE_MATRIX.md')
 for item in REQUIRED:
  if item.lower() not in txt.lower(): failures.append(f'feature matrix missing: {item}')
 print(json.dumps({'ok':not failures,'failures':failures}, indent=2))
 return 0 if not failures else 1
if __name__=='__main__': raise SystemExit(main())

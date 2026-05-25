#!/usr/bin/env python3
import argparse, json, re, sys
from pathlib import Path

def main():
 ap=argparse.ArgumentParser(); ap.add_argument('--log', required=True); ap.add_argument('--expect', choices=['pass','fail'], default='pass'); args=ap.parse_args()
 txt=Path(args.log).read_text(errors='ignore')
 findings=[]
 missing=re.findall(r'Ingestion failed source_id="([^"]+)" error=Not found: Notebook ([^ ]+) not found', txt)
 if missing:
  notebooks=sorted(set(n for _,n in missing)); findings.append({'code':'missing_notebook_ingestion','count':len(missing),'notebooks':notebooks[:5]})
 if 'input length exceeds the context length' in txt:
  findings.append({'code':'ollama_context_length_projection_failure'})
 if re.search(r'Citations:\s*0 valid,\s*[1-9][0-9]* filtered', txt) or '0 valid, 6 filtered' in txt:
  findings.append({'code':'zero_valid_citations_filtered'})
 if 'Skipping native dense search warmup because native indexing is disabled' in txt:
  findings.append({'code':'native_dense_disabled_bm25_fallback'})
 if re.search(r'[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}(,\s*[0-9a-f]{8}-[0-9a-f]{4}){20,}', txt):
  findings.append({'code':'uuid_flood'})
 bad=bool(findings)
 ok=(not bad) if args.expect=='pass' else bad
 print(json.dumps({'ok':ok,'expected':args.expect,'finding_count':len(findings),'findings':findings}, indent=2))
 return 0 if ok else 1
if __name__=='__main__': raise SystemExit(main())

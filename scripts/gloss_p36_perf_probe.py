#!/usr/bin/env python3
import argparse, json, pathlib, subprocess, time, os, resource

def run(cmd, cwd, timeout=60):
    t0=time.time(); p=subprocess.run(cmd, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
    return {'cmd':cmd,'returncode':p.returncode,'elapsed_ms':round((time.time()-t0)*1000,2),'stdout_tail':p.stdout[-2000:],'stderr_tail':p.stderr[-2000:]}

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--repo', default='.'); ap.add_argument('--run-id', required=True); ap.add_argument('--quick', action='store_true'); ap.add_argument('--out')
    a=ap.parse_args(); repo=pathlib.Path(a.repo).resolve(); out=pathlib.Path(a.out) if a.out else repo/'docs/codex-runs'/a.run_id/'perf.receipt.json'
    receipt={'run_id':a.run_id,'recorded_utc':time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),'certified':False,'mode':'quick' if a.quick else 'full','hardware':{'hostname':os.uname().nodename,'platform':os.uname().sysname+' '+os.uname().release},'measurements':{},'commands':[],'failures':[],'warnings':[]}
    try:
        receipt['commands'].append(run(['python3','validation/gloss_retrieval_decision_gate.py','--repo',str(repo)], repo, 120))
        receipt['measurements']['retrieval_decision_gate_ms']=receipt['commands'][-1]['elapsed_ms']
        receipt['commands'].append(run(['python3','validation/gloss_generation_receipt_gate.py','--repo',str(repo)], repo, 120))
        receipt['measurements']['generation_receipt_gate_ms']=receipt['commands'][-1]['elapsed_ms']
        receipt['measurements']['max_rss_kb']=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        if a.quick:
            receipt['warnings'].append('quick mode is non-certifying; no live first-token or idle memory measurement')
        else:
            receipt['failures'].append('full live performance probe not implemented for headless controller; run headed/manual timing and update receipt')
    except Exception as e:
        receipt['failures'].append(str(e))
    out.parent.mkdir(parents=True, exist_ok=True); out.write_text(json.dumps(receipt, indent=2)+"\n")
    print(json.dumps(receipt, indent=2))
    return 0 if not receipt['failures'] else 1
if __name__=='__main__': raise SystemExit(main())

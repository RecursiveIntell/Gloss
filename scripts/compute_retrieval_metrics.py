#!/usr/bin/env python3
import json, sys, pathlib
rows=json.loads(pathlib.Path(sys.argv[1]).read_text()); out={'queries':len(rows),'top1_hits':0,'overlap5_sum':0.0,'mrr10_sum':0.0}
for r in rows:
    exp=set(r.get('expected_source_ids',[])); ranked=r.get('ranked_source_ids') or r.get('actual_source_ids',[])
    if ranked and ranked[0] in exp: out['top1_hits']+=1
    if exp:
        out['overlap5_sum'] += len(exp.intersection(ranked[:5]))/len(exp)
        rr=0.0
        for i,s in enumerate(ranked[:10],1):
            if s in exp: rr=1.0/i; break
        out['mrr10_sum'] += rr
n=max(1,out['queries']); out['top1_hit_rate']=out['top1_hits']/n; out['overlap_at_5']=out['overlap5_sum']/n; out['mrr_at_10']=out['mrr10_sum']/n
print(json.dumps(out, indent=2))

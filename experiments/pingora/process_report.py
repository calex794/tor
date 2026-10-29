#!/usr/bin/env python3
import json,sys
prefix,before_count,after_count,cpus,rate,round_no,scheme=sys.argv[1:]
before=json.load(open(prefix+".before.json"))
after=json.load(open(prefix+".after.json"))
client=json.load(open(prefix+".client.json"))
if before["ticks_per_second"]!=after["ticks_per_second"]:raise SystemExit("clock rate changed")
delta={}
for name in ("nginx","pingora","axum"):
 a,b=before["components"][name],after["components"][name]
 if set(a["pids"])!=set(b["pids"]):raise SystemExit("pid changed "+name)
 delta[name]={
  "cpu_seconds":round((b["cpu_ticks"]-a["cpu_ticks"])/before["ticks_per_second"],3),
  "rss_kib":b["rss_kib"],
  "context_switches":(b["voluntary_switches"]+b["involuntary_switches"])-(a["voluntary_switches"]+a["involuntary_switches"])
 }
result={"vcpu":int(cpus),"rate":int(rate),"round":int(round_no),"scheme":scheme,"client":client,"processes":delta,"upstream_accepted_connections":int(after_count)-int(before_count)-1}
if result["upstream_accepted_connections"]<0:raise SystemExit("negative upstream connections")
with open(prefix+".profile.json","w") as f:json.dump(result,f,indent=2)
print("PROFILE_RESULT",json.dumps({"vcpu":cpus,"rate":rate,"scheme":scheme,"round":round_no,"goodput":client["goodput_rps"],"p95":client["p95_ms"],"proxy_cpu":delta[scheme]["cpu_seconds"],"upstream_new":result["upstream_accepted_connections"],"errors":client["errors"],"dropped":client["dropped"]}))

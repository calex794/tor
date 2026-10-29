#!/usr/bin/env python3
import json,os,subprocess,sys
raw=subprocess.check_output(["docker","top",sys.argv[1],"-eo","pid,comm"],text=True)
groups={"nginx":[],"pingora":[],"axum":[]}
for line in raw.splitlines()[1:]:
 p=line.split()
 if len(p)!=2 or not p[0].isdigit():continue
 name=("nginx" if p[1].startswith("nginx") else "pingora" if p[1].startswith("tor-pingora") else "axum" if p[1].startswith("axum_fixture") else None)
 if name:groups[name].append(int(p[0]))
out={"ticks_per_second":os.sysconf("SC_CLK_TCK"),"components":{}}
for name,pids in groups.items():
 if not pids:raise SystemExit("missing process "+name)
 total={"cpu_ticks":0,"rss_kib":0,"voluntary_switches":0,"involuntary_switches":0}
 for pid in pids:
  f=open("/proc/%d/stat"%pid).read().rsplit(") ",1)[1].split()
  total["cpu_ticks"]+=int(f[11])+int(f[12])
  for row in open("/proc/%d/status"%pid):
   for key,field in (("VmRSS:","rss_kib"),("voluntary_ctxt_switches:","voluntary_switches"),("nonvoluntary_ctxt_switches:","involuntary_switches")):
    if row.startswith(key):total[field]+=int(row.split()[1])
 total["pids"]=pids
 out["components"][name]=total
print(json.dumps(out))

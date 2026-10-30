#!/usr/bin/env python3
"""Publish reviewed source blobs only; connector owns commit/ref updates.
No deployment, branch changes, credentials in output, or environment capture.
"""
import json
import os
from pathlib import Path
import subprocess
from urllib.error import HTTPError
from urllib.request import Request, urlopen

REPO='calex794/tor'
BRANCH='feat/f/rec-feedback-system-20261031'
BASE='https://api.github.com/repos/'+REPO
head=os.environ['REC_HEAD_SHA']
if os.environ.get('GITHUB_REPOSITORY')!=REPO or os.environ.get('REC_HEAD_BRANCH')!=BRANCH:
    raise SystemExit('refusing unrelated repository or branch')

def api(path,data=None):
    body=None if data is None else json.dumps(data).encode()
    req=Request(BASE+path,data=body,headers={'Authorization':'Bearer '+os.environ['GH_TOKEN'],'Accept':'application/vnd.github+json','Content-Type':'application/json','X-GitHub-Api-Version':'2022-11-28'},method='GET' if data is None else 'POST')
    try:
        with urlopen(req,timeout=60) as response:
            return json.load(response)
    except HTTPError as error:
        details=json.loads(error.read().decode())
        raise SystemExit(f'GitHub {error.code}: {details.get("message","request rejected")}') from error

current=api('/git/ref/heads/'+BRANCH)['object']['sha']
if current!=head: raise SystemExit('branch advanced; no writes made')
base_tree=api('/git/commits/'+head)['tree']['sha']
changed=set(subprocess.check_output(['git','diff','--name-only','HEAD'],text=True).splitlines())
new=subprocess.check_output(['git','ls-files','--others','--exclude-standard'],text=True).splitlines()
allowed=('crates/','docs/','migrations/','tests/','tools/','.github/workflows/')
changed.update(path for path in new if path.startswith(allowed) and not path.startswith(('tools/node_modules/','tests/node_modules/')))
entries=[]
for name in sorted(changed):
    if not name.startswith(allowed): raise SystemExit('unexpected changed file: '+name)
    path=Path(name)
    if not path.exists():
        entry={'path':name,'mode':'100644','type':'blob','sha':None}
    else:
        if path.stat().st_size>2_000_000: raise SystemExit('unexpected large source: '+name)
        content=path.read_text(encoding='utf-8')
        blob=api('/git/blobs',{'encoding':'utf-8','content':content})['sha']
        entry={'path':name,'mode':'100644','type':'blob','sha':blob}
    entries.append(entry)
    print('REC_BLOB '+json.dumps(entry),flush=True)
# Workflow blobs require connector workflow authority when incorporated into a
# Git tree. The CI token intentionally cannot update workflows, commits or refs.
print('REC_PREPARED_ENTRIES '+json.dumps({'base_sha':head,'base_tree':base_tree,'entries':entries}),flush=True)

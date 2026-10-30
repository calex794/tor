#!/usr/bin/env python3
"""Publish only source Git objects for review. Never update a branch or deploy.
Token is supplied only to this CI step and is never printed or persisted.
"""
import json
import os
from pathlib import Path
import subprocess
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
    with urlopen(req,timeout=60) as response:
        return json.load(response)

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
        entries.append({'path':name,'mode':'100644','type':'blob','sha':None})
        continue
    if path.stat().st_size>2_000_000: raise SystemExit('unexpected large source: '+name)
    content=path.read_text(encoding='utf-8')
    blob=api('/git/blobs',{'encoding':'utf-8','content':content})['sha']
    entries.append({'path':name,'mode':'100644','type':'blob','sha':blob})
if not entries:
    print('REC_PREPARED_TREE',base_tree,'BASE',head,'FILES',0)
else:
    tree=api('/git/trees',{'base_tree':base_tree,'tree':entries})['sha']
    print('REC_PREPARED_TREE',tree,'BASE',head,'FILES',len(entries))

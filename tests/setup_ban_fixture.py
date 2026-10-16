import pathlib,os,json,sys,sqlite3,hashlib,hmac,time
# Synthetic local accounts only. Run once before starting the fixture server, then with --seed.
base=pathlib.Path(os.environ.get('BANS_TEST_FIXTURE','/tmp/mitch-bans-audit'))
assert str(base).startswith('/tmp/mitch-bans-')
repo=pathlib.Path(__file__).resolve().parents[1]
(base/'data').mkdir(parents=True,exist_ok=True)
if not (base/'webserver').exists(): (base/'webserver').symlink_to(repo/'webserver')
if not (base/'data/id_secret.key').exists(): (base/'data/id_secret.key').write_bytes(os.urandom(32))
(base/'data/admins.json').write_text(json.dumps({'owners':['banadmin@example.com'],'admins':[],'coOwners':[]}))
if len(sys.argv)<2: raise SystemExit()
secret=(base/'data/id_secret.key').read_bytes()
c=sqlite3.connect(base/'data/mitchpro.db'); now=int(time.time()*1000)
for email,username in [('banadmin@example.com','fixture_admin'),('banstudent@student.rjuhsd.us','fixture_student'),('banneighbor@student.rjuhsd.us','fixture_neighbor')]:
 for length in (24,32):
  raw='e'+hashlib.sha256(email.encode()).hexdigest()[:length]
  sid=raw+'.'+hmac.new(secret,raw.encode(),'sha256').hexdigest()[:16]
  c.execute('INSERT OR REPLACE INTO names(sid,email,norm_email,created_at) VALUES(?,?,?,?)',(sid,email,email,now))
 c.execute('INSERT OR REPLACE INTO users(email,username,nickname,display_name,has_completed_tutorial,created_at,updated_at) VALUES(?,?,?,?,?,?,?)',(email,username,username,username,1,now,now))
 c.execute('INSERT OR REPLACE INTO profiles(email,data,updated_at) VALUES(?,?,?)',(email,json.dumps({'username':username,'displayName':username,'hasCompletedTutorial':True}),now))
c.commit();c.close()

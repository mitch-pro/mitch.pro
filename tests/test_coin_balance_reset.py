import importlib.util
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

spec=importlib.util.spec_from_file_location('reset_coins',Path(__file__).parents[1]/'tools/reset-coin-balances.py')
module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class BalanceResetTest(unittest.TestCase):
    def test_backup_atomic_reset_and_no_repeat(self):
        with tempfile.TemporaryDirectory() as folder:
            db=Path(folder)/'data.sqlite3'
            with sqlite3.connect(db) as c:
                c.executescript('CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT,updated_at INTEGER);CREATE TABLE coins(email TEXT PRIMARY KEY,balance REAL,lifetime_earned REAL,updated_at INTEGER);CREATE TABLE users(email TEXT PRIMARY KEY,coins REAL,updated_at INTEGER);CREATE TABLE json_documents(path TEXT PRIMARY KEY,content TEXT,updated_at INTEGER);CREATE TABLE cosmetics(email TEXT,data TEXT);')
                c.executemany('INSERT INTO coins VALUES(?,?,?,0)', [('owner',1e29,80),('member',-30,70)])
                c.executemany('INSERT INTO users VALUES(?,?,0)', [('owner',100),('new',40)])
                c.execute('INSERT INTO json_documents VALUES(?,?,0)', ('/data/coins.json',json.dumps({'old':200})))
                c.execute('INSERT INTO cosmetics VALUES(?,?)', ('member','neon_purple'))
            self.assertEqual(module.reset(db)['nonzero'],2)
            with self.assertRaises(ValueError):module.reset(db,folder,True,False)
            result=module.reset(db,Path(folder)/'backups',True,True)
            self.assertEqual(result['accounts'],3)
            self.assertEqual(result['nonzero'],0)
            self.assertEqual(Path(result['backup']).stat().st_mode & 0o777,0o600)
            with sqlite3.connect(result['backup']) as c:self.assertEqual(c.execute('SELECT balance FROM coins WHERE email="owner"').fetchone()[0],1e29)
            with sqlite3.connect(db) as c:
                self.assertEqual(c.execute('SELECT lifetime_earned FROM coins WHERE email="member"').fetchone()[0],70)
                self.assertEqual(c.execute('SELECT data FROM cosmetics').fetchone()[0],'neon_purple')
                self.assertEqual(json.loads(c.execute('SELECT content FROM json_documents').fetchone()[0]),{'old':0})
                c.execute('UPDATE coins SET balance=25 WHERE email="member"')
            self.assertTrue(module.reset(db,Path(folder)/'backups',True,True)['already_applied'])
            with sqlite3.connect(db) as c:self.assertEqual(c.execute('SELECT balance FROM coins WHERE email="member"').fetchone()[0],25)
if __name__=='__main__':unittest.main()

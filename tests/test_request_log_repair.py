import importlib.util
from pathlib import Path
import sqlite3
import tempfile
import unittest

spec=importlib.util.spec_from_file_location('repair_logs',Path(__file__).parents[1]/'tools/repair-request-log-table.py')
module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class RequestLogRepairTest(unittest.TestCase):
    def test_corrupt_log_leaf_preserves_accounts_and_salvages_other_logs(self):
        with tempfile.TemporaryDirectory() as folder:
            source=Path(folder)/'damaged.sqlite3'
            target=Path(folder)/'clean.sqlite3'
            with sqlite3.connect(source) as c:
                c.executescript('CREATE TABLE users(email TEXT PRIMARY KEY,coins REAL,secret BLOB);CREATE TABLE app_logs(id INTEGER PRIMARY KEY AUTOINCREMENT,ts INTEGER,message TEXT);CREATE INDEX idx_logs_ts ON app_logs(ts);')
                c.execute('INSERT INTO users VALUES(?,?,?)',('member@example.com',123.25,b'private-value'))
                c.executemany('INSERT INTO app_logs(ts,message) VALUES(?,?)',((i,'sample request '+str(i)+'x'*80) for i in range(3000)))
                page_size=c.execute('PRAGMA page_size').fetchone()[0]
                page=c.execute("SELECT pageno FROM dbstat WHERE name='app_logs' AND pagetype='leaf' LIMIT 1").fetchone()[0]
            # Corrupt only a request-log page in this disposable fixture.
            with source.open('r+b') as f:
                f.seek((page-1)*page_size)
                f.write(b'\x00')
            original=source.read_bytes()
            result=module.repair(source,target)
            self.assertEqual(source.read_bytes(),original)
            self.assertEqual(result['integrity'],'ok')
            self.assertGreater(result['log_rows_recovered'],2000)
            self.assertLess(result['log_rows_recovered'],3000)
            self.assertGreater(result['unreadable_log_id_slots'],0)
            with sqlite3.connect(target) as c:
                self.assertEqual(c.execute('PRAGMA integrity_check').fetchall(),[('ok',)])
                self.assertEqual(c.execute('SELECT * FROM users').fetchone(),('member@example.com',123.25,b'private-value'))
                self.assertEqual(c.execute("SELECT seq FROM sqlite_sequence WHERE name='app_logs'").fetchone()[0],3000)
            with self.assertRaises(FileExistsError):module.repair(source,target)

    def test_refuses_damage_in_an_account_table(self):
        with tempfile.TemporaryDirectory() as folder:
            source=Path(folder)/'damaged.sqlite3'
            with sqlite3.connect(source) as c:
                c.executescript('CREATE TABLE users(id INTEGER PRIMARY KEY,email TEXT);CREATE TABLE app_logs(id INTEGER PRIMARY KEY AUTOINCREMENT,message TEXT);')
                c.executemany('INSERT INTO users(email) VALUES(?)',[(str(i)+'x'*80,) for i in range(500)])
                page_size=c.execute('PRAGMA page_size').fetchone()[0]
                page=c.execute("SELECT pageno FROM dbstat WHERE name='users' AND pagetype='leaf' LIMIT 1").fetchone()[0]
            with source.open('r+b') as f:
                f.seek((page-1)*page_size)
                f.write(b'\x00')
            with self.assertRaises((RuntimeError,sqlite3.DatabaseError)):module.repair(source,Path(folder)/'refused.sqlite3')
if __name__=='__main__':unittest.main()

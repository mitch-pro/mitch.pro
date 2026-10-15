#!/usr/bin/env python3
"""Back up and reset ALL stored balances once. Stop web writers before --apply.

Example: python3 tools/reset-coin-balances.py --database /path/data/mitchpro.db \
  --backup-dir /path/private-backups --apply --writers-stopped

Accounts, purchases, daily streaks, and lifetime earnings are preserved.
No credentials or individual balances are printed. The default is read-only.
"""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import sqlite3
import time

RESET_KEY = 'mitchcoins-balance-reset-2026-10-15'

def reset(database, backup_dir=None, apply=False, writers_stopped=False):
    database = Path(database).resolve(strict=True)
    conn = sqlite3.connect(f'file:{database}?mode={"rw" if apply else "ro"}', uri=True, timeout=30)
    conn.row_factory = sqlite3.Row
    try:
        marker = conn.execute('SELECT value FROM metadata WHERE key=?', (RESET_KEY,)).fetchone()
        if marker:
            return {'already_applied': True, 'reset': json.loads(marker[0])}
        before = dict(conn.execute('SELECT COUNT(*) AS accounts, SUM(balance != 0) AS nonzero FROM coins').fetchone())
        if not apply:
            return {'dry_run': True, **before}
        if not writers_stopped or backup_dir is None:
            raise ValueError('--apply requires --writers-stopped and --backup-dir')
        backup_dir = Path(backup_dir).resolve()
        backup_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(backup_dir, 0o700)
        stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')
        backup = backup_dir / f'pre-coins-reset-{stamp}.sqlite3'
        fd = os.open(backup, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        os.close(fd)
        with sqlite3.connect(backup) as saved:
            conn.backup(saved)
            if saved.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
                raise RuntimeError('Backup integrity check failed; no balances changed')
        now = int(time.time() * 1000)
        conn.execute('BEGIN IMMEDIATE')
        try:
            # Keep rows even at zero: startup's legacy backfill uses INSERT OR IGNORE.
            conn.execute('UPDATE coins SET balance=0, updated_at=?', (now,))
            conn.execute('INSERT OR IGNORE INTO coins(email,balance,lifetime_earned,updated_at) SELECT email,0,0,? FROM users', (now,))
            conn.execute('UPDATE users SET coins=0, updated_at=?', (now,))
            # Also zero the retired snapshot so removed accounts cannot be reimported with old coins.
            for row in conn.execute("SELECT path,content FROM json_documents WHERE path LIKE '%coins.json'").fetchall():
                value = json.loads(row['content'])
                if isinstance(value, dict):
                    conn.execute('UPDATE json_documents SET content=?,updated_at=? WHERE path=?', (json.dumps({key:0 for key in value}),now,row['path']))
            remaining = conn.execute('SELECT COUNT(*) FROM coins WHERE balance != 0').fetchone()[0]
            if remaining or conn.execute('SELECT COUNT(*) FROM users WHERE coins != 0').fetchone()[0]:
                raise RuntimeError('Reset verification failed')
            result = {'accounts': conn.execute('SELECT COUNT(*) FROM coins').fetchone()[0], 'nonzero': remaining, 'completed_at': stamp, 'backup': str(backup)}
            conn.execute('INSERT INTO metadata(key,value,updated_at) VALUES(?,?,?)', (RESET_KEY,json.dumps(result),now))
            conn.commit()
        except BaseException:
            conn.rollback()
            raise
        return {'applied': True, **result}
    finally:
        conn.close()

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--database', required=True)
    parser.add_argument('--backup-dir')
    parser.add_argument('--apply', action='store_true')
    parser.add_argument('--writers-stopped', action='store_true')
    args = parser.parse_args()
    print(json.dumps(reset(args.database,args.backup_dir,args.apply,args.writers_stopped),indent=2))

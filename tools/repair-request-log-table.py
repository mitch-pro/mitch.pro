#!/usr/bin/env python3
"""Create a verified clean SQLite copy when only app_logs is damaged.

Use an offline snapshot as --source. This tool NEVER changes or replaces it.
Healthy table contents are copied and compared by SHA-256; readable request
logs are salvaged by primary-key ranges. Keep the damaged source for recovery.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import pickle
import sqlite3


def quote(name):
    return '"' + name.replace('"', '""') + '"'


def digest_rows(conn, name):
    digest = hashlib.sha256()
    count = 0
    for row in conn.execute('SELECT * FROM '+quote(name)+' ORDER BY rowid'):
        data = pickle.dumps(tuple(row), protocol=4)
        digest.update(len(data).to_bytes(8, 'big'))
        digest.update(data)
        count += 1
    return count, digest.hexdigest()


def repair(source, destination):
    source = Path(source).resolve(strict=True)
    destination = Path(destination).resolve()
    if source == destination:
        raise ValueError('Destination must differ from the source')
    # Exclusive creation prevents accidentally overwriting another database.
    fd = os.open(destination, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    os.close(fd)
    src = sqlite3.connect('file:'+str(source)+'?mode=ro', uri=True)
    dst = sqlite3.connect(destination)
    report = {'healthy_tables': {}, 'log_rows_recovered': 0, 'unreadable_log_id_slots': 0}
    try:
        schema = src.execute('SELECT type,name,tbl_name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY rowid').fetchall()
        tables = [(name, sql) for kind,name,_,sql in schema if kind == 'table' and not name.startswith('sqlite_')]
        if 'app_logs' not in [name for name,_ in tables]:
            raise ValueError('Source has no request-log table')
        for name, _ in tables:
            if name != 'app_logs':
                if src.execute('PRAGMA integrity_check('+quote(name)+')').fetchall() != [('ok',)]:
                    raise RuntimeError('Refusing recovery: damage outside app_logs in '+name)
        page_size = src.execute('PRAGMA page_size').fetchone()[0]
        dst.execute('PRAGMA page_size='+str(page_size))
        dst.execute('PRAGMA journal_mode=DELETE')
        for _, sql in tables:
            dst.execute(sql)
        for name, _ in tables:
            if name == 'app_logs':
                continue
            columns = [row[1] for row in src.execute('PRAGMA table_info('+quote(name)+')')]
            insert = 'INSERT INTO '+quote(name)+' ('+','.join(map(quote,columns))+') VALUES ('+','.join('?' for _ in columns)+')'
            cursor = src.execute('SELECT * FROM '+quote(name)+' ORDER BY rowid')
            while rows := cursor.fetchmany(1000):
                dst.executemany(insert, rows)
            expected = digest_rows(src,name)
            actual = digest_rows(dst,name)
            if actual != expected:
                raise RuntimeError('Healthy table verification failed: '+name)
            report['healthy_tables'][name] = {'rows': actual[0], 'sha256': actual[1]}
        seq = src.execute("SELECT seq FROM sqlite_sequence WHERE name='app_logs'").fetchone()
        if not seq or not isinstance(seq[0], int) or seq[0] < 0:
            raise RuntimeError('Cannot determine request-log ID range safely')
        columns = [r[1] for r in src.execute('PRAGMA table_info(app_logs)')]
        id_pos = columns.index('id')
        log_insert = 'INSERT OR IGNORE INTO app_logs ('+','.join(map(quote,columns))+') VALUES ('+','.join('?' for _ in columns)+')'
        def salvage(low, high):
            if low > high:
                return
            damaged = False
            try:
                cursor = src.execute('SELECT * FROM app_logs WHERE id BETWEEN ? AND ? ORDER BY id', (low, high))
            except sqlite3.DatabaseError:
                damaged = True
            while not damaged:
                try:
                    rows = cursor.fetchmany(200)
                except sqlite3.DatabaseError:
                    damaged = True
                    break
                if not rows:
                    break
                if any(not isinstance(row[id_pos],int) or not low <= row[id_pos] <= high for row in rows):
                    damaged = True
                    break
                # Destination errors are fatal, never treated as unreadable source rows.
                dst.executemany(log_insert, rows)
            if damaged:
                if low == high:
                    report['unreadable_log_id_slots'] += 1
                    if report['unreadable_log_id_slots'] > 10000:
                        raise RuntimeError('Too many damaged log ID slots; refusing automatic salvage')
                else:
                    middle = (low + high) // 2
                    salvage(low, middle)
                    salvage(middle + 1, high)
        salvage(1, seq[0])
        report['log_rows_recovered'] = dst.execute('SELECT COUNT(*) FROM app_logs').fetchone()[0]
        if any(name == 'sqlite_sequence' for kind,name,_,_ in schema if kind == 'table'):
            dst.execute('DELETE FROM sqlite_sequence')
            dst.executemany('INSERT INTO sqlite_sequence(name,seq) VALUES(?,?)', src.execute('SELECT name,seq FROM sqlite_sequence').fetchall())
        # Rebuild indexes from recovered rows rather than copying damaged pages.
        for kind, _, _, sql in schema:
            if kind in ('index','trigger','view'):
                dst.execute(sql)
        for pragma in ('user_version','application_id'):
            dst.execute('PRAGMA '+pragma+'='+str(src.execute('PRAGMA '+pragma).fetchone()[0]))
        dst.commit()
        if dst.execute('PRAGMA integrity_check').fetchall() != [('ok',)]:
            raise RuntimeError('Rebuilt database failed its integrity check')
        for name, expected in report['healthy_tables'].items():
            if digest_rows(dst,name) != (expected['rows'],expected['sha256']):
                raise RuntimeError('Final content verification failed: '+name)
        report['integrity'] = 'ok'
        return report
    finally:
        dst.close()
        src.close()

if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source',required=True)
    parser.add_argument('--destination',required=True)
    args=parser.parse_args()
    result=repair(args.source,args.destination)
    # Counts and hashes only; no user data or log messages on stdout.
    print(json.dumps(result,indent=2))

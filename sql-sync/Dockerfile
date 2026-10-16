# Litestream sidecar: continuously replicates mitchpro.db from the shared
# tmpfs ramdisk (fast, volatile) to the HDD-backed ./data mount (durable).
# See entrypoint.sh for the one-time legacy-database seed on first boot.
FROM litestream/litestream:0.5.14

# sqlite3 CLI is not in the base image - entrypoint.sh needs it for the
# one-time `.backup` seed (SQLite's online-backup API, safe against the
# legacy database still being actively written to during that seed).
USER root
RUN apt-get update && apt-get install -y --no-install-recommends sqlite3 \
    && rm -rf /var/lib/apt/lists/*

COPY entrypoint.sh /entrypoint.sh
COPY litestream.yml /etc/litestream.yml
RUN chmod +x /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]

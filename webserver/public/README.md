Anything placed in this directory is served at the site root, unauthenticated —
a file at `webserver/public/foo.html` is reachable at `/foo.html` with no
login required, regardless of what the rest of the site requires.

This is checked ahead of the normal auth gate, so it will also shadow a
same-named file that would otherwise live at `webserver/<path>`.

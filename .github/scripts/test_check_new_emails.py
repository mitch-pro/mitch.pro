"""Regression tests for check_new_emails.py. Run: python3 -m unittest discover .github/scripts"""
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "check_new_emails.py"
# Built at runtime so this file does not itself contain a non-allowlisted address.
REAL = "tyler.someone" + "@" + "gmail.com"


def git(repo, *args):
    subprocess.run(["git", "-C", str(repo), "-c", "user.email=t@t", "-c", "user.name=t", *args],
                   check=True, capture_output=True)


class CheckNewEmails(unittest.TestCase):
    def run_check(self, files):
        """Commit `files` ({path: text}) on top of an empty base commit and
        return the checker's exit code."""
        with tempfile.TemporaryDirectory() as d:
            repo = Path(d)
            git(repo, "init", "-q")
            (repo / "base.txt").write_text("base\n")
            git(repo, "add", "-A")
            git(repo, "commit", "-qm", "base")
            base = subprocess.run(["git", "-C", d, "rev-parse", "HEAD"], check=True,
                                  capture_output=True, text=True).stdout.strip()
            for name, text in files.items():
                (repo / name).parent.mkdir(parents=True, exist_ok=True)
                (repo / name).write_text(text)
            git(repo, "add", "-A")
            git(repo, "commit", "-qm", "pr")
            # The script reads the allowlist next to itself; run it in place.
            return subprocess.run(["python3", str(SCRIPT), base, "HEAD"], cwd=d,
                                  capture_output=True, text=True).returncode

    def test_control_plain_email_fails(self):
        self.assertEqual(self.run_check({"a.txt": f'x = "{REAL}"\n'}), 1)

    def test_allowlisted_passes(self):
        self.assertEqual(self.run_check({"a.txt": 'x = "test1@example.com"\n'}), 0)

    def test_plus_plus_line_does_not_hide_later_lines(self):
        self.assertEqual(self.run_check({"a.txt": f"++ x\n{REAL}\n"}), 1)

    def test_email_on_plus_plus_line_itself(self):
        self.assertEqual(self.run_check({"a.txt": f"++ {REAL}\n"}), 1)

    def test_lookalike_lock_file_names_are_scanned(self):
        for name in ("evil-package-lock.json", "sub/Cargo.lock", "x/rust/Cargo.lock"):
            with self.subTest(name=name):
                self.assertEqual(self.run_check({name: f"{REAL}\n"}), 1)

    def test_gitattributes_diff_off_does_not_hide(self):
        self.assertEqual(self.run_check({".gitattributes": "*.txt -diff\n",
                                         "a.txt": f"{REAL}\n"}), 1)

    def test_email_ok_marker_no_longer_skips(self):
        self.assertEqual(self.run_check({"a.txt": f"{REAL} # email-ok\n"}), 1)

    def test_filename_with_spaces(self):
        self.assertEqual(self.run_check({"dir x/a b.txt": f"{REAL}\n"}), 1)


if __name__ == "__main__":
    unittest.main()

import io
import subprocess
import sys
from pathlib import Path

from claude_sessions import cli
from claude_sessions.core import paths


def test_hook_bad_stdin_logs_and_returns_0(monkeypatch):
    monkeypatch.setattr("sys.stdin", io.TextIOWrapper(io.BytesIO(b"{")))
    assert cli.main(["hook"]) == 0
    log = Path(paths.state_dir(), paths.ERROR_LOG)
    assert "JSONDecodeError" in log.read_text()


def test_cli_does_not_import_textual_or_rich():
    code = "import claude_sessions.cli, sys; assert not {'textual', 'rich'} & set(sys.modules)"
    subprocess.run([sys.executable, "-c", code], check=True)

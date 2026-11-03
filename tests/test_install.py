import json
import os
from pathlib import Path

from claude_sessions import cli
from claude_sessions.claude import install, statusline

CS = "/opt/bin/cs"
FOREIGN = {"hooks": [{"type": "command", "command": "notify-me"}]}


def sample():
    return {
        "model": "opus",
        "hooks": {"Stop": [FOREIGN], "Custom": [FOREIGN]},
        "statusLine": {"type": "command", "command": "mine.sh", "padding": 2},
    }


def test_merge_keeps_foreign_hooks_on_same_event():
    new, _ = install.merge(sample(), CS, None)
    assert new["hooks"]["Stop"][0] == FOREIGN
    assert new["hooks"]["Stop"][1]["hooks"][0]["command"] == f"{CS} hook"
    assert new["hooks"]["Stop"][1]["hooks"][0]["timeout"] == 10
    assert set(install.EVENTS) <= set(new["hooks"])
    assert len(install.EVENTS) == 11


def test_merge_is_idempotent():
    once, saved = install.merge(sample(), CS, None)
    twice, saved2 = install.merge(once, CS, saved)
    assert twice == once
    assert saved2 == saved


def test_merge_wraps_user_statusline_keeping_padding():
    new, saved = install.merge(sample(), CS, None)
    assert new["statusLine"] == {"type": "command", "command": f"{CS} statusline", "padding": 2}
    assert saved == {"type": "command", "command": "mine.sh", "padding": 2}


def test_merge_creates_statusline_when_absent():
    new, saved = install.merge({}, CS, None)
    assert new["statusLine"] == {"type": "command", "command": f"{CS} statusline"}
    assert saved is None


def test_unmerge_restores_original_exactly():
    original = sample()
    new, saved = install.merge(json.loads(json.dumps(original)), CS, None)
    assert install.unmerge(new, saved) == original


def test_unmerge_without_saved_removes_our_statusline():
    new, saved = install.merge({"model": "opus"}, CS, None)
    assert install.unmerge(new, saved) == {"model": "opus"}


def test_a_cs_path_with_a_space_is_quoted_and_still_ours():
    cs = "/opt/my tools/cs"
    new, _ = install.merge({}, cs, None)
    command = new["hooks"]["Stop"][0]["hooks"][0]["command"]
    assert command == "'/opt/my tools/cs' hook" and install.is_ours(command)
    assert new["statusLine"]["command"] == "'/opt/my tools/cs' statusline"
    assert install.unmerge(new, None) == {}


def test_is_ours():
    assert install.is_ours('"/x/cs" hook')
    assert install.is_ours("/x/cs statusline")
    assert not install.is_ours('"/x/cs" panel')
    assert not install.is_ours("/x/other hook")
    assert not install.is_ours('"unterminated hook')
    assert not install.is_ours("")


def settings_path(tmp_path):
    return tmp_path / "claude" / "settings.json"


def saved_statusline():
    return Path(statusline.saved_statusline_path())


def test_install_writes_through_symlink(tmp_path, monkeypatch, capsys):
    real = tmp_path / "dots" / "settings.json"
    real.parent.mkdir()
    real.write_text(json.dumps(sample()))
    link = settings_path(tmp_path)
    link.parent.mkdir()
    link.symlink_to(real)
    monkeypatch.setattr("sys.argv", [CS])
    assert cli.main(["install"]) == 0
    assert link.is_symlink()
    assert "hooks" in json.loads(real.read_text())
    assert real.read_text().endswith("}\n")
    assert (real.parent / "settings.json.before-claude-sessions").exists()
    assert json.loads(saved_statusline().read_text()) == sample()["statusLine"]
    capsys.readouterr()
    assert cli.main(["install"]) == 0
    assert "already installed" in capsys.readouterr().out
    assert cli.main(["uninstall"]) == 0
    assert json.loads(real.read_text()) == sample()
    assert link.is_symlink()
    assert not os.path.exists(statusline.saved_statusline_path())


def test_install_on_missing_settings_creates_file(tmp_path, monkeypatch):
    monkeypatch.setattr("sys.argv", [CS])
    assert cli.main(["install"]) == 0
    data = json.loads(settings_path(tmp_path).read_text())
    assert set(data["hooks"]) == set(install.EVENTS)
    assert not os.path.exists(statusline.saved_statusline_path())
    assert not (tmp_path / "claude" / "settings.json.before-claude-sessions").exists()
    assert cli.main(["uninstall"]) == 0
    assert json.loads(settings_path(tmp_path).read_text()) == {}


def test_install_refuses_invalid_json(tmp_path, monkeypatch, capsys):
    path = settings_path(tmp_path)
    path.parent.mkdir()
    path.write_text("{oops")
    monkeypatch.setattr("sys.argv", [CS])
    assert cli.main(["install"]) == 1
    assert "settings.json" in capsys.readouterr().err
    assert path.read_text() == "{oops"
    assert not os.path.exists(statusline.saved_statusline_path())


def test_merge_repoints_stale_path():
    old = {
        "hooks": {
            e: [FOREIGN, {"hooks": [{"type": "command", "command": '"/old/cs" hook'}]}]
            for e in install.EVENTS[:4]
        },
        "statusLine": {"type": "command", "command": '"/old/cs" statusline', "padding": 1},
    }
    saved = {"type": "command", "command": "mine.sh"}
    new, kept = install.merge(old, CS, saved)
    for event in install.EVENTS:
        ours = [h for g in new["hooks"][event] for h in g["hooks"] if install.is_ours(h["command"])]
        assert [h["command"] for h in ours] == [f"{CS} hook"]
    assert new["statusLine"] == {"type": "command", "command": f"{CS} statusline", "padding": 1}
    assert kept == saved


def test_install_reports_updated_path(monkeypatch, capsys):
    monkeypatch.setattr("sys.argv", ["/old/cs"])
    cli.main(["install"])
    monkeypatch.setattr("sys.argv", [CS])
    capsys.readouterr()
    assert cli.main(["install"]) == 0
    out = capsys.readouterr().out
    assert f"hooks: updated to {CS}" in out
    assert f"status line: updated to {CS}" in out


def test_mixed_foreign_and_ours_group_round_trips():
    original = {
        "hooks": {
            "Stop": [
                {"hooks": [FOREIGN["hooks"][0], {"type": "command", "command": '"/x/cs" hook'}]}
            ]
        }
    }
    new, saved = install.merge(original, CS, None)
    assert len(new["hooks"]["Stop"]) == 1
    out = install.unmerge(new, saved)
    assert out["hooks"]["Stop"] == [{"hooks": [FOREIGN["hooks"][0]]}]


def test_unmerge_drops_only_the_containers_it_emptied():
    ours = {"hooks": [{"type": "command", "command": f"{CS} hook"}]}
    assert install.unmerge({"hooks": {"Stop": [ours]}}, None) == {}
    assert install.unmerge({"hooks": {"Stop": [ours], "X": []}}, None) == {"hooks": {"X": []}}
    # Nothing of ours was in it: an empty container is not ours to drop
    line = {"type": "command", "command": f"{CS} statusline"}
    assert install.unmerge({"hooks": {}, "statusLine": line}, None) == {"hooks": {}}


def test_reinstall_with_saved_and_user_statusline(tmp_path, monkeypatch):
    path = settings_path(tmp_path)
    path.parent.mkdir()
    path.write_text(json.dumps({"statusLine": {"type": "command", "command": "first.sh"}}))
    monkeypatch.setattr("sys.argv", [CS])
    cli.main(["install"])
    data = json.loads(path.read_text())
    data["statusLine"] = {"type": "command", "command": "second.sh"}
    path.write_text(json.dumps(data))
    cli.main(["install"])
    assert json.loads(saved_statusline().read_text())["command"] == "second.sh"
    cli.main(["uninstall"])
    assert json.loads(path.read_text())["statusLine"]["command"] == "second.sh"


def test_statusline_already_ours_before_first_install(tmp_path, monkeypatch):
    path = settings_path(tmp_path)
    path.parent.mkdir()
    line = {"type": "command", "command": f"{CS} statusline"}
    path.write_text(json.dumps({"statusLine": line}))
    monkeypatch.setattr("sys.argv", [CS])
    cli.main(["install"])
    assert json.loads(path.read_text())["statusLine"] == line
    assert not os.path.exists(statusline.saved_statusline_path())
    cli.main(["uninstall"])
    assert "statusLine" not in json.loads(path.read_text())


def test_uninstall_says_restored_for_empty_saved(tmp_path, monkeypatch, capsys):
    path = settings_path(tmp_path)
    path.parent.mkdir()
    path.write_text('{"statusLine": {}}')
    monkeypatch.setattr("sys.argv", [CS])
    cli.main(["install"])
    capsys.readouterr()
    cli.main(["uninstall"])
    assert "yours restored" in capsys.readouterr().out
    assert json.loads(path.read_text()) == {"statusLine": {}}


def test_malformed_shapes_are_refused(tmp_path, monkeypatch, capsys):
    path = settings_path(tmp_path)
    path.parent.mkdir()
    monkeypatch.setattr("sys.argv", [CS])
    for bad in ('{"hooks": []}', '{"hooks": {"Stop": {}}}', '{"hooks": {"Stop": [{}]}}'):
        path.write_text(bad)
        assert cli.main(["install"]) == 1
        assert cli.main(["uninstall"]) == 1
        assert "settings.json" in capsys.readouterr().err
        assert path.read_text() == bad


def test_unreadable_settings_fail_cleanly(tmp_path, monkeypatch, capsys):
    path = settings_path(tmp_path)
    path.mkdir(parents=True)  # a directory where the file should be
    monkeypatch.setattr("sys.argv", [CS])
    assert cli.main(["install"]) == 1
    assert "error:" in capsys.readouterr().err


def test_install_keeps_file_mode_and_leaves_no_temp(tmp_path, monkeypatch):
    path = settings_path(tmp_path)
    path.parent.mkdir()
    path.write_text('{"note": "é"}')
    path.chmod(0o600)
    monkeypatch.setattr("sys.argv", [CS])
    cli.main(["install"])
    assert path.stat().st_mode & 0o777 == 0o600
    assert "é" in path.read_text(encoding="utf-8")
    assert not [n for n in os.listdir(path.parent) if n.startswith(".tmp-")]


def test_uninstall_refuses_a_broken_saved_statusline(tmp_path, monkeypatch, capsys):
    path = settings_path(tmp_path)
    path.parent.mkdir()
    path.write_text(json.dumps(sample()))
    monkeypatch.setattr("sys.argv", [CS])
    cli.main(["install"])
    installed = path.read_text()
    saved_statusline().write_text("{not json")
    capsys.readouterr()
    assert cli.main(["uninstall"]) == 1
    assert "statusline.json" in capsys.readouterr().err
    assert path.read_text() == installed
    assert saved_statusline().read_text() == "{not json"

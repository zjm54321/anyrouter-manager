"""Execute publication shell with a fake Docker CLI; no engine/token/network."""
import ast
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[2]


def condition(expression, values):
    """Evaluate the workflow's limited boolean/comparison grammar, not arbitrary code."""
    tree = ast.parse(expression.replace("&&", " and ").replace("||", " or "), mode="eval")

    def visit(node):
        if isinstance(node, ast.Expression):
            return visit(node.body)
        if isinstance(node, ast.Constant):
            return node.value
        if isinstance(node, (ast.Name, ast.Attribute)):
            return values[ast.unparse(node)]
        if isinstance(node, ast.BoolOp):
            operands = [visit(value) for value in node.values]
            assert isinstance(node.op, (ast.And, ast.Or))
            return all(operands) if isinstance(node.op, ast.And) else any(operands)
        if isinstance(node, ast.Compare):
            assert len(node.ops) == 1 and isinstance(node.ops[0], (ast.Eq, ast.NotEq))
            equal = visit(node.left) == visit(node.comparators[0])
            return equal if isinstance(node.ops[0], ast.Eq) else not equal
        raise AssertionError("unsupported workflow expression")

    return visit(tree)


def run_body(step):
    body = re.search(r"^        run: \|\n((?:          .*\n)+)", step, re.M).group(1)
    return "\n".join(line[10:] for line in body.splitlines())


def main():
    workflow = (ROOT / ".github/workflows/container.yml").read_text()
    validate, publish = workflow.split("  publish:", 1)
    assert workflow.startswith("name: Public container\n")
    assert "group: public-container-${{ github.ref }}" in workflow
    validate_job = validate.split("  validate:\n", 1)[1]
    assert not re.search(r"^    if:", validate_job, re.M)
    triggers = workflow.split("permissions:", 1)[0]
    assert re.search(r"^  push:\n(?:    #.*\n)*    branches: \[main\]$", triggers, re.M)
    assert re.search(r"^  workflow_dispatch:\s*$", triggers, re.M)
    assert "inputs:" not in triggers and "paths:" not in triggers
    assert "pull_request_target" not in workflow
    expression = " ".join(re.search(r"    if: >-\n((?:      .*\n)+)", publish).group(1).split())
    cases = 0
    for private in (True, False):
        for event in ("push", "workflow_dispatch", "pull_request", "pull_request_target", "schedule"):
            for ref, default in (("main", "main"), ("main", "other"), ("feature", "main")):
                values = {"false": False, "github.event.repository.private": private,
                          "github.event_name": event, "github.ref": "refs/heads/" + ref,
                          "github.ref_name": ref, "github.event.repository.default_branch": default}
                assert condition(expression, values) == (not private and ref == default == "main" and event in ("push", "workflow_dispatch"))
                cases += 1
    for gate in re.findall(r"^        if: (.*)$", validate, re.M):
        for event in ("pull_request", "push", "workflow_dispatch"):
            for ref in ("refs/heads/main", "refs/heads/feature", "refs/pull/1/merge"):
                assert condition(gate, {"github.event_name": event, "github.ref": ref}) == (
                    event == "pull_request" or ref != "refs/heads/main")
    assert "packages: write" not in validate and "GITHUB_TOKEN" not in validate
    assert "secrets." not in validate and "docker/login-action" not in validate
    assert "permissions:\n  contents: read\n" in validate
    assert workflow.count("packages: write") == 1
    assert workflow.count("persist-credentials: false") == 2
    assert "packages: write" in publish and "secrets.GITHUB_TOKEN" in publish
    assert "secrets." not in publish.replace("secrets.GITHUB_TOKEN", "")
    assert "cache-to:" not in workflow and "upload-artifact" not in workflow
    assert workflow.count("DOCKER_BUILD_RECORD_UPLOAD: false") == 2
    assert workflow.count("DOCKER_BUILD_SUMMARY: false") == 2
    assert workflow.count("load: true") == 2 and workflow.count("push: false") == 2
    assert "push: true" not in workflow and "continue-on-error" not in workflow
    assert "production supervisor/browser gate" in validate
    assert "python3 -B tests/container/ci_smoke.py anyrouter-manager:ci" in validate
    steps = publish.split("      - ")[1:]
    tag_step = next(step for step in steps if "name: Tag and push" in step)
    smoke_step = next(step for step in steps if "id: smoke" in step)
    assert "exit 2" in smoke_step and "::error::" in smoke_step
    assert publish.count("uses: docker/build-push-action@") == 1
    assert publish.index("id: smoke") < publish.index("docker/login-action") < publish.index("docker push")
    assert publish.index("Require an existing public GHCR package") < publish.index("docker/login-action")
    assert publish.index("docker push") < publish.index("Verify published package exists and is public")
    assert publish.count("run: python3 -B docker/ghcr_visibility.py\n") == 2
    assert "REPOSITORY: ${{ github.repository }}" in publish
    assert "TESTED_IMAGE_ID: ${{ steps.smoke.outputs.tested_image_id }}" in tag_step
    for step in steps:
        if any(term in step for term in ("id: image", "GH_TOKEN:", "docker/login-action", "name: Tag and push")):
            gate = re.search(r"^        if: (.*)$", step, re.M).group(1)
            for ready in ("true", "false", "", "TRUE"):
                assert condition(gate, {"steps.smoke.outputs.publish_ready": ready}) == (ready == "true")
    revision = "b" * 40
    image_id = "sha256:" + "a" * 64
    with tempfile.TemporaryDirectory(prefix="workflow-mock-") as directory:
        path = Path(directory)
        docker = path / "docker"
        docker.write_text(f"#!{sys.executable}\n" + '''import json,os,sys
with open(os.environ["MOCK_LOG"], "a") as stream:
    stream.write(json.dumps(sys.argv[1:]) + "\\n")
if sys.argv[1:3] == ["image", "inspect"]:
    print(os.environ["MOCK_IMAGE_ID"])
elif sys.argv[1] == "push" and os.environ.get("MOCK_PUSH_FAIL") == "1":
    sys.exit(1)
''')
        docker.chmod(0o700)
        output, log = path / "outputs", path / "calls"
        env = dict(os.environ, PATH=directory + os.pathsep + os.environ["PATH"], MOCK_LOG=str(log),
                   IMAGE="ghcr.io/fixtureowner/fixturerepo", REVISION=revision, TESTED_IMAGE_ID=image_id,
                   MOCK_IMAGE_ID=image_id, GITHUB_OUTPUT=str(output), REPOSITORY="FixtureOwner/FixtureRepo")
        python = path / "python3"
        python.write_text(f"#!{sys.executable}\nimport os,sys\nsys.exit(int(os.environ['MOCK_SMOKE_STATUS']))\n")
        python.chmod(0o700)
        for status in (0, 1, 2):
            env["MOCK_SMOKE_STATUS"] = str(status)
            result = subprocess.run(["bash", "-e", "-o", "pipefail", "-c", run_body(smoke_step)], env=env, capture_output=True)
            assert result.returncode == status
            if status == 2:
                assert b"::error::" in result.stdout and b"No login or publication" in result.stdout
        name_step = next(step for step in steps if "id: image" in step)
        name_script = re.search(r"^        run: (.*)$", name_step, re.M).group(1)
        assert subprocess.run(["bash", "-e", "-c", name_script], env=env, capture_output=True).returncode == 0
        assert output.read_text() == "name=ghcr.io/fixtureowner/fixturerepo\n"
        for scenario in ("pass", "changed_id", "missing_tested_id", "push_failed"):
            log.write_text("")
            env["TESTED_IMAGE_ID"] = "" if scenario == "missing_tested_id" else image_id
            env["MOCK_IMAGE_ID"] = image_id if scenario != "changed_id" else "sha256:" + "c" * 64
            env["MOCK_PUSH_FAIL"] = "1" if scenario == "push_failed" else "0"
            result = subprocess.run(["bash", "-e", "-o", "pipefail", "-c", run_body(tag_step)], env=env, capture_output=True)
            calls = [json.loads(line) for line in log.read_text().splitlines()]
            assert (result.returncode == 0) == (scenario == "pass")
            assert not any(call[0] == "build" for call in calls)
            if scenario in ("changed_id", "missing_tested_id"):
                assert len(calls) == 1  # mutated local tag cannot be published
            else:
                assert calls[1:3] == [["tag", image_id, env["IMAGE"] + ":sha-" + revision],
                                      ["tag", image_id, env["IMAGE"] + ":main"]]
                assert calls[3] == ["push", env["IMAGE"] + ":sha-" + revision]
                assert len(calls) == (5 if scenario == "pass" else 4)
                if scenario == "pass":
                    assert calls[4] == ["push", env["IMAGE"] + ":main"]
    dockerfile = (ROOT / "Dockerfile").read_text()
    assert re.findall(r"^ARG (\w+)", dockerfile, re.M) == ["TARGETARCH"]
    assert not re.search(r"(?im)^(?:ARG|ENV).*?(?:password|root_key|gh_token|github_token)", dockerfile)
    manifest = tomllib.loads((ROOT / "backend/Cargo.toml").read_text())
    lock = tomllib.loads((ROOT / "backend/Cargo.lock").read_text())
    # Stable Rust supports edition 2024 (>=1.85) and lockfile v4. Actual locked
    # dependency MSRV compatibility is still enforced by the remote Cargo build.
    assert manifest["package"]["edition"] == "2024" and lock["version"] == 4
    assert "FROM rust:bookworm AS backend" in dockerfile
    print(json.dumps({"workflow_contract": "passed", "checks": 6, "event_condition_cases": cases,
                      "mock_docker_only": True}))


if __name__ == "__main__":
    main()

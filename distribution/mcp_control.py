"""Bounded installed-product control through the actual stdio MCP interface."""
import json
import queue
import subprocess
import threading
import time


class Session:
    def __init__(self, binary, data, work, environment, output):
        self.stderr = (output / "mcp.stderr.txt").open("wb")
        self.transcript = (output / "mcp-transcript.jsonl").open("w", encoding="utf-8")
        self.stdout_log = (output / "mcp.stdout.jsonl").open("w", encoding="utf-8")
        self.process = subprocess.Popen([str(binary), "--data-dir", str(data), "mcp"], cwd=work, env=environment,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr, text=True, encoding="utf-8")
        self.responses = queue.Queue()
        self.sequence = 0
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        try:
            for line in self.process.stdout:
                self.stdout_log.write(line)
                self.stdout_log.flush()
                self.responses.put(json.loads(line))
        except BaseException as error:
            self.responses.put(error)
        finally:
            self.responses.put(RuntimeError("Installed MCP server closed stdout"))

    def send(self, message):
        self.transcript.write(json.dumps({"direction": "request", "message": message}) + "\n")
        self.transcript.flush()
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def request(self, method, params):
        self.sequence += 1
        self.send({"jsonrpc": "2.0", "id": self.sequence, "method": method, "params": params})
        try:
            response = self.responses.get(timeout=15)
        except queue.Empty as error:
            raise RuntimeError(f"Installed MCP request timed out: {method}") from error
        if isinstance(response, BaseException):
            raise response
        self.transcript.write(json.dumps({"direction": "response", "message": response}) + "\n")
        self.transcript.flush()
        if response.get("id") != self.sequence or "error" in response:
            raise RuntimeError(f"Installed MCP protocol error: {response}")
        return response["result"]

    def call(self, name, arguments):
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError") is not False or "structuredContent" not in result:
            raise RuntimeError(f"Installed MCP operation failed ({name}): {result}")
        return result["structuredContent"]

    def close(self):
        try:
            if self.process.stdin and not self.process.stdin.closed:
                self.process.stdin.close()
            try:
                status = self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
                raise RuntimeError("Installed MCP server did not shut down after stdin EOF")
            if status:
                raise RuntimeError(f"Installed MCP server exited with status {status}")
        finally:
            self.reader.join(timeout=2)
            self.process.stdout.close()
            self.stdout_log.close()
            self.stderr.close()
            self.transcript.close()


def terminal(session, operation, arguments, select, label):
    deadline = time.monotonic() + 45
    while True:
        value = session.call(operation, arguments)
        state = select(value)
        if state and state["status"] != "running":
            return value, state
        if time.monotonic() > deadline:
            raise RuntimeError(f"Installed MCP {label} did not reach a terminal state")
        time.sleep(.1)


def verify_mcp(binary, data, work, environment, output, before, source, expected_stdout_sha256):
    source.write_bytes((work / "retained-original.mjs").read_bytes())
    session = Session(binary, data, work, environment, output)
    try:
        initialize = session.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "hunin-installed-control", "version": "1"}})
        if initialize.get("serverInfo", {}).get("name") != "hunin":
            raise RuntimeError("Installed MCP initialization did not identify Hunin")
        session.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        catalog = session.request("tools/list", {})
        names = {tool["name"] for tool in catalog["tools"]}
        for required in ("capabilities", "runs.start", "profiles.hotspots", "analyses.register", "comparisons.create"):
            if required not in names:
                raise RuntimeError(f"Installed MCP tool discovery omitted {required}")
        capabilities = session.call("capabilities", {})
        started = session.call("runs.start", {"name": "installed MCP rerun", "runtime": "node", "command": ["node", str(source)], "cwd": str(work),
                                              "timeout_seconds": 30, "expected_exit_code": 0, "expected_stdout_sha256": expected_stdout_sha256})
        run_id = started["run"]["id"]
        recording, run = terminal(session, "runs.get", {"run_id": run_id}, lambda value: value["run"], "recording")
        if run["status"] != "completed" or run["correctness"]["passed"] is not True or recording["summary"]["sample_count"] == 0:
            raise RuntimeError(f"Installed MCP recording failed: {recording}")
        package = output / "analyzer-source"
        package.mkdir()
        code = package / "analyze.mjs"
        code.write_text("import {randomUUID} from 'node:crypto';\nlet text='';for await(const chunk of process.stdin)text+=chunk;const input=JSON.parse(text);\n"
                        "const span=input.records.find(row=>row.kind==='span'&&row.name==='installed work');const context=input.records.find(row=>row.kind==='context'&&row.id===span?.context_id);\n"
                        "if(!context||!span)throw Error('Required explicit application context and work evidence missing');\n"
                        "console.log(JSON.stringify({schema_version:1,kind:'relation',id:randomUUID(),run_id:input.run.id,producer_id:'installed-source-analyzer',pid:span.pid,tid:span.tid,timestamp_us:span.timestamp_us,basis:'derived',from:context.id,to:span.id,relation:'contains explicit measured work',evidence:[context.id,span.id]}));\n")
        requirements = {"runtime": "node", "source_fingerprint": run["source"]["fingerprint"]}
        for field in ("executable_sha256", "runtime_version"):
            if run["source"].get(field):
                requirements[field] = run["source"][field]
        registered = session.call("analyses.register", {"name": "installed context relationship", "version": "1", "cwd": str(package), "command": ["node", "analyze.mjs"], "requirements": requirements})["analysis"]
        # The retained package, not this disposable authoring directory, must run.
        code.rename(package / "original-not-executed.mjs")
        attempt = session.call("analyses.run", {"run_id": run_id, "analysis_id": registered["id"]})
        _, accepted = terminal(session, "analyses.results", {"run_id": run_id}, lambda value: next((row for row in value["results"] if row["id"] == attempt["result_set_id"]), None), "analyzer")
        if accepted["status"] != "accepted" or accepted.get("selected") is not True:
            raise RuntimeError(f"Managed installed analyzer did not publish accepted output: {accepted}")
        relations = session.call("contexts.query", {"run_id": run_id})["relations"]
        derived = next((row for row in relations if row.get("relation") == "contains explicit measured work"), None)
        if not derived or derived["basis"] != "derived":
            raise RuntimeError("Accepted analyzer relationship is missing from common queries")
        evidence = session.call("evidence.get", {"run_id": run_id, "ids": derived["evidence"]})
        if {row["id"] for row in evidence["records"]} != set(derived["evidence"]):
            raise RuntimeError("Installed analyzer evidence cannot be resolved")
        replay = session.call("analyses.run", {"run_id": run_id, "analysis_id": registered["id"]})
        _, replayed = terminal(session, "analyses.results", {"run_id": run_id}, lambda value: next((row for row in value["results"] if row["id"] == replay["result_set_id"]), None), "analyzer replay")
        if replayed["status"] != "replayed" or replayed["result_set_id"] != accepted["result_set_id"]:
            raise RuntimeError(f"Installed analyzer retry lost idempotency: {replayed}")
        comparison = session.call("comparisons.create", {"before": before, "after": run_id})
        if comparison["comparable"] is not True or comparison["correctness"]["after"]["passed"] is not True:
            raise RuntimeError(f"Equivalent installed CLI and MCP workloads are not comparable: {comparison}")
        final = session.call("runs.get", {"run_id": run_id})
        return {"run_id": run_id, "summary": final["summary"], "tool_count": len(names), "host": capabilities["host"],
                "analysis_id": registered["id"], "accepted_result_set_id": accepted["result_set_id"], "replay_status": replayed["status"],
                "resolved_evidence_ids": derived["evidence"], "comparison_comparable": comparison["comparable"]}
    finally:
        session.close()

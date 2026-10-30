"""Owned installed MCP child, bounded requests, transcripts and EOF shutdown."""

import json
import queue
import subprocess
import threading


class Session:
    def __init__(self, binary, data, work, environment, output):
        self.stderr = (output / "mcp.stderr.txt").open("wb")
        self.transcript = (output / "mcp-transcript.jsonl").open("w", encoding="utf-8")
        self.stdout_log = (output / "mcp.stdout.jsonl").open("w", encoding="utf-8")
        self.process = subprocess.Popen(
            [str(binary), "--data-dir", str(data), "mcp"],
            cwd=work,
            env=environment,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.stderr,
            text=True,
            encoding="utf-8",
        )
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
        self.transcript.write(
            json.dumps({"direction": "request", "message": message}) + "\n"
        )
        self.transcript.flush()
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def request(self, method, params):
        self.sequence += 1
        self.send(
            {"jsonrpc": "2.0", "id": self.sequence, "method": method, "params": params}
        )
        try:
            response = self.responses.get(timeout=15)
        except queue.Empty as error:
            raise RuntimeError(f"Installed MCP request timed out: {method}") from error
        if isinstance(response, BaseException):
            raise response
        self.transcript.write(
            json.dumps({"direction": "response", "message": response}) + "\n"
        )
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
                raise RuntimeError(
                    "Installed MCP server did not shut down after stdin EOF"
                )
            if status:
                raise RuntimeError(f"Installed MCP server exited with status {status}")
        finally:
            self.reader.join(timeout=2)
            self.process.stdout.close()
            self.stdout_log.close()
            self.stderr.close()
            self.transcript.close()

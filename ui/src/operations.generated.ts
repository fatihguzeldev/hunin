// Generated from protocol/operations.json and response.schema.json. Run npm run generate; do not edit.
export type Run = {
  "id": string;
  "name": string;
  "status": string;
  "command": Array<string>;
  "cwd": string;
  "runtime": string;
  "started_at_us": number;
  "finished_at_us": number | null;
  "exit_code": number | null;
  "diagnostics": Array<string>;
  "source": unknown;
  "correctness": unknown;
};

export type Summary = {
  "sample_count": number;
  "total_weight": number | null;
  "weight_unit": string;
  "span_count": number;
  "metric_count": number;
  "relation_count": number;
  "diagnostics": Array<string>;
};

export type RunPage = {
  "runs": Array<Run>;
  "next_cursor": string | null;
};

export type RunResult = {
  "run": Run;
};

export type RunDetails = {
  "run": Run;
  "summary": Summary;
};

export type ImportResult = {
  "run": Run;
  "accepted": boolean;
  "summary": Summary;
};

export type ReanalysisResult = {
  "accepted": boolean;
  "diagnostics": Array<string>;
  "summary": Summary;
};

export type OperationError = {
  "code": string;
  "message": string;
  "details"?: unknown;
};

export type HttpSuccess = {
  "result": unknown;
};

export type HttpFailure = {
  "error": OperationError;
};

export type McpToolSuccess = {
  "content": Array<{
  "type": "text";
  "text": string;
}>;
  "structuredContent": unknown;
  "isError": false;
};

export type McpToolFailure = {
  "content": Array<{
  "type": "text";
  "text": string;
}>;
  "isError": true;
};

export type OperationInputs = {
  "capabilities": {

};
  "runs.list": {
  "limit"?: number;
  "cursor"?: string;
};
  "runs.get": {
  "run_id": string;
};
  "runs.start": {
  "command": Array<string>;
  "name"?: string;
  "cwd"?: string;
  "runtime"?: string;
  "timeout_seconds"?: number;
  "expected_exit_code"?: number;
  "env"?: Record<string, unknown>;
  "input_files"?: Array<string>;
  "expected_stdout"?: string;
  "expected_stdout_sha256"?: string;
};
  "runs.cancel": {
  "run_id": string;
};
  "runs.remove": {
  "run_id": string;
};
  "profiles.hotspots": {
  "run_id": string;
  "limit"?: number;
  "weight_unit"?: string;
};
  "profiles.tree": {
  "run_id": string;
  "weight_unit"?: string;
  "limit"?: number;
  "cursor"?: string;
  "path"?: Array<string>;
  "depth_limit"?: number;
};
  "timeline.query": {
  "run_id": string;
  "limit"?: number;
  "cursor"?: string;
  "start_us"?: number;
  "end_us"?: number;
  "kind"?: string;
  "pid"?: number;
  "alignment"?: "aligned" | "unmapped" | "all";
};
  "resources.query": {
  "run_id": string;
  "limit"?: number;
  "cursor"?: string;
  "start_us"?: number;
  "end_us"?: number;
  "point_limit"?: number;
};
  "contexts.query": {
  "run_id": string;
  "limit"?: number;
  "cursor"?: string;
};
  "evidence.get": {
  "run_id": string;
  "ids": Array<string>;
};
  "artifacts.list": {
  "run_id": string;
  "limit"?: number;
  "cursor"?: string;
};
  "artifacts.read": {
  "run_id": string;
  "artifact_id": string;
  "offset"?: number;
  "limit"?: number;
};
  "source.read": {
  "run_id": string;
  "path": string;
  "line"?: number;
  "count"?: number;
  "code_id"?: string;
};
  "recordings.import": {
  "path": string;
  "format": string;
  "run_id"?: string;
  "name"?: string;
};
  "comparisons.create": {
  "before": string;
  "after": string;
};
  "analyses.list": {
  "run_id"?: string;
  "limit"?: number;
  "cursor"?: string;
};
  "analyses.register": {
  "name": string;
  "command": Array<string>;
  "cwd"?: string;
  "requirements": Record<string, unknown>;
  "version"?: string;
  "files"?: Array<string>;
  "dependencies"?: Record<string, unknown>;
};
  "analyses.run": {
  "run_id": string;
  "analysis_id": string;
  "timeout_seconds"?: number;
};
  "analyses.results": {
  "run_id": string;
  "limit"?: number;
  "cursor"?: string;
};
  "scenarios.list": {
  "limit"?: number;
  "cursor"?: string;
};
  "scenarios.save": {
  "id"?: string;
  "name": string;
  "command": Array<string>;
  "cwd"?: string;
  "runtime"?: string;
  "timeout_seconds"?: number;
  "expected_exit_code"?: number;
  "env"?: Record<string, unknown>;
  "input_files"?: Array<string>;
  "expected_stdout"?: string;
  "expected_stdout_sha256"?: string;
};
  "scenarios.run": {
  "scenario_id": string;
  "name"?: string;
};
  "analyses.cancel": {
  "execution_id": string;
};
  "recordings.reanalyze": {
  "run_id": string;
  "artifact_id": string;
};
};

export type OperationOutputs = {
  "runs.list": RunPage;
  "runs.get": RunDetails;
  "runs.start": RunResult;
  "runs.cancel": RunResult;
  "runs.remove": {
  "removed": true;
};
  "artifacts.list": {
  "artifacts": Array<{
  "id": string;
  "name": string;
  "size": number;
  "sha256": string;
  "format": string;
}>;
  "next_cursor": string | null;
};
  "artifacts.read": {
  "content": string;
  "encoding": "utf8_lossy";
  "next_offset": number | null;
  "size": number;
};
  "recordings.import": ImportResult;
  "scenarios.run": RunResult;
  "recordings.reanalyze": ReanalysisResult;
};

function isOutputObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function isRun(value: unknown): value is Run {
  return (isOutputObject(value) && Object.hasOwn(value, "id") && Object.hasOwn(value, "name") && Object.hasOwn(value, "status") && Object.hasOwn(value, "command") && Object.hasOwn(value, "cwd") && Object.hasOwn(value, "runtime") && Object.hasOwn(value, "started_at_us") && Object.hasOwn(value, "finished_at_us") && Object.hasOwn(value, "exit_code") && Object.hasOwn(value, "diagnostics") && Object.hasOwn(value, "source") && Object.hasOwn(value, "correctness") && (typeof value["id"] === "string") && (typeof value["name"] === "string") && (typeof value["status"] === "string") && (Array.isArray(value["command"]) && value["command"].every(item => (typeof item === "string"))) && (typeof value["cwd"] === "string") && (typeof value["runtime"] === "string") && (typeof value["started_at_us"] === "number" && Number.isInteger(value["started_at_us"]) && value["started_at_us"] >= 0) && ((typeof value["finished_at_us"] === "number" && Number.isInteger(value["finished_at_us"]) && value["finished_at_us"] >= 0) || (value["finished_at_us"] === null)) && ((typeof value["exit_code"] === "number" && Number.isInteger(value["exit_code"])) || (value["exit_code"] === null)) && (Array.isArray(value["diagnostics"]) && value["diagnostics"].every(item => (typeof item === "string"))) && true && true);
}

export function isSummary(value: unknown): value is Summary {
  return (isOutputObject(value) && Object.hasOwn(value, "sample_count") && Object.hasOwn(value, "total_weight") && Object.hasOwn(value, "weight_unit") && Object.hasOwn(value, "span_count") && Object.hasOwn(value, "metric_count") && Object.hasOwn(value, "relation_count") && Object.hasOwn(value, "diagnostics") && (typeof value["sample_count"] === "number" && Number.isInteger(value["sample_count"]) && value["sample_count"] >= 0) && ((typeof value["total_weight"] === "number" && Number.isFinite(value["total_weight"])) || (value["total_weight"] === null)) && (typeof value["weight_unit"] === "string") && (typeof value["span_count"] === "number" && Number.isInteger(value["span_count"]) && value["span_count"] >= 0) && (typeof value["metric_count"] === "number" && Number.isInteger(value["metric_count"]) && value["metric_count"] >= 0) && (typeof value["relation_count"] === "number" && Number.isInteger(value["relation_count"]) && value["relation_count"] >= 0) && (Array.isArray(value["diagnostics"]) && value["diagnostics"].every(item => (typeof item === "string"))));
}

export function isRunPage(value: unknown): value is RunPage {
  return (isOutputObject(value) && Object.hasOwn(value, "runs") && Object.hasOwn(value, "next_cursor") && (Array.isArray(value["runs"]) && value["runs"].every(item => isRun(item))) && ((typeof value["next_cursor"] === "string") || (value["next_cursor"] === null)));
}

export function isRunResult(value: unknown): value is RunResult {
  return (isOutputObject(value) && Object.hasOwn(value, "run") && isRun(value["run"]));
}

export function isRunDetails(value: unknown): value is RunDetails {
  return (isOutputObject(value) && Object.hasOwn(value, "run") && Object.hasOwn(value, "summary") && isRun(value["run"]) && isSummary(value["summary"]));
}

export function isImportResult(value: unknown): value is ImportResult {
  return (isOutputObject(value) && Object.hasOwn(value, "run") && Object.hasOwn(value, "accepted") && Object.hasOwn(value, "summary") && isRun(value["run"]) && (typeof value["accepted"] === "boolean") && isSummary(value["summary"]));
}

export function isReanalysisResult(value: unknown): value is ReanalysisResult {
  return (isOutputObject(value) && Object.hasOwn(value, "accepted") && Object.hasOwn(value, "diagnostics") && Object.hasOwn(value, "summary") && (typeof value["accepted"] === "boolean") && (Array.isArray(value["diagnostics"]) && value["diagnostics"].every(item => (typeof item === "string"))) && isSummary(value["summary"]));
}

export function isOperationError(value: unknown): value is OperationError {
  return (isOutputObject(value) && Object.hasOwn(value, "code") && Object.hasOwn(value, "message") && (typeof value["code"] === "string") && (typeof value["message"] === "string") && (!Object.hasOwn(value, "details") || true));
}

export function isHttpSuccess(value: unknown): value is HttpSuccess {
  return (isOutputObject(value) && Object.hasOwn(value, "result") && true);
}

export function isHttpFailure(value: unknown): value is HttpFailure {
  return (isOutputObject(value) && Object.hasOwn(value, "error") && isOperationError(value["error"]));
}

export function isMcpToolSuccess(value: unknown): value is McpToolSuccess {
  return (isOutputObject(value) && Object.hasOwn(value, "content") && Object.hasOwn(value, "structuredContent") && Object.hasOwn(value, "isError") && (Array.isArray(value["content"]) && value["content"].every(item => (isOutputObject(item) && Object.hasOwn(item, "type") && Object.hasOwn(item, "text") && (item["type"] === "text") && (typeof item["text"] === "string")))) && true && (value["isError"] === false));
}

export function isMcpToolFailure(value: unknown): value is McpToolFailure {
  return (isOutputObject(value) && Object.hasOwn(value, "content") && Object.hasOwn(value, "isError") && (Array.isArray(value["content"]) && value["content"].every(item => (isOutputObject(item) && Object.hasOwn(item, "type") && Object.hasOwn(item, "text") && (item["type"] === "text") && (typeof item["text"] === "string")))) && (value["isError"] === true));
}

// Only declared output schemas are checked while the catalog is migrated.
export function validateOperationOutput(name: string, value: unknown): void {
  switch (name) {
    case "runs.list":
      if (!isRunPage(value)) throw new Error("Hunin returned an invalid runs.list result.");
      break;
    case "runs.get":
      if (!isRunDetails(value)) throw new Error("Hunin returned an invalid runs.get result.");
      break;
    case "runs.start":
      if (!isRunResult(value)) throw new Error("Hunin returned an invalid runs.start result.");
      break;
    case "runs.cancel":
      if (!isRunResult(value)) throw new Error("Hunin returned an invalid runs.cancel result.");
      break;
    case "runs.remove":
      if (!(isOutputObject(value) && Object.hasOwn(value, "removed") && (value["removed"] === true))) throw new Error("Hunin returned an invalid runs.remove result.");
      break;
    case "artifacts.list":
      if (!(isOutputObject(value) && Object.hasOwn(value, "artifacts") && Object.hasOwn(value, "next_cursor") && (Array.isArray(value["artifacts"]) && value["artifacts"].every(item => (isOutputObject(item) && Object.hasOwn(item, "id") && Object.hasOwn(item, "name") && Object.hasOwn(item, "size") && Object.hasOwn(item, "sha256") && Object.hasOwn(item, "format") && (typeof item["id"] === "string") && (typeof item["name"] === "string") && (typeof item["size"] === "number" && Number.isInteger(item["size"]) && item["size"] >= 0) && (typeof item["sha256"] === "string") && (typeof item["format"] === "string")))) && ((typeof value["next_cursor"] === "string") || (value["next_cursor"] === null)))) throw new Error("Hunin returned an invalid artifacts.list result.");
      break;
    case "artifacts.read":
      if (!(isOutputObject(value) && Object.hasOwn(value, "content") && Object.hasOwn(value, "encoding") && Object.hasOwn(value, "next_offset") && Object.hasOwn(value, "size") && (typeof value["content"] === "string") && (value["encoding"] === "utf8_lossy") && ((typeof value["next_offset"] === "number" && Number.isInteger(value["next_offset"]) && value["next_offset"] >= 0) || (value["next_offset"] === null)) && (typeof value["size"] === "number" && Number.isInteger(value["size"]) && value["size"] >= 0))) throw new Error("Hunin returned an invalid artifacts.read result.");
      break;
    case "recordings.import":
      if (!isImportResult(value)) throw new Error("Hunin returned an invalid recordings.import result.");
      break;
    case "scenarios.run":
      if (!isRunResult(value)) throw new Error("Hunin returned an invalid scenarios.run result.");
      break;
    case "recordings.reanalyze":
      if (!isReanalysisResult(value)) throw new Error("Hunin returned an invalid recordings.reanalyze result.");
      break;
  }
}

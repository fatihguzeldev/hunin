// Generated from protocol/operations.json. Run npm run generate; do not edit.
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
};

function isOutputObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

// Only declared output schemas are checked while the catalog is migrated.
export function validateOperationOutput(name: string, value: unknown): void {
  switch (name) {
    case "artifacts.list":
      if (!(isOutputObject(value) && Object.hasOwn(value, "artifacts") && Object.hasOwn(value, "next_cursor") && (Array.isArray(value["artifacts"]) && value["artifacts"].every(item => (isOutputObject(item) && Object.hasOwn(item, "id") && Object.hasOwn(item, "name") && Object.hasOwn(item, "size") && Object.hasOwn(item, "sha256") && Object.hasOwn(item, "format") && (typeof item["id"] === "string") && (typeof item["name"] === "string") && (typeof item["size"] === "number" && Number.isInteger(item["size"]) && item["size"] >= 0) && (typeof item["sha256"] === "string") && (typeof item["format"] === "string")))) && ((typeof value["next_cursor"] === "string") || (value["next_cursor"] === null)))) throw new Error("Hunin returned an invalid artifacts.list result.");
      break;
    case "artifacts.read":
      if (!(isOutputObject(value) && Object.hasOwn(value, "content") && Object.hasOwn(value, "encoding") && Object.hasOwn(value, "next_offset") && Object.hasOwn(value, "size") && (typeof value["content"] === "string") && (value["encoding"] === "utf8_lossy") && ((typeof value["next_offset"] === "number" && Number.isInteger(value["next_offset"]) && value["next_offset"] >= 0) || (value["next_offset"] === null)) && (typeof value["size"] === "number" && Number.isInteger(value["size"]) && value["size"] >= 0))) throw new Error("Hunin returned an invalid artifacts.read result.");
      break;
  }
}

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
};
  "scenarios.list": {

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

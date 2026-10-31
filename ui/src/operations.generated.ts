// Generated from protocol/operations.json and response.schema.json. Run npm run generate; do not edit.
import type { Observation } from "./record.generated.js";
export type { Observation } from "./record.generated.js";

export type Project = {
  "id": string;
  "name": string;
  "store_path": string;
  "workspace_roots": Array<string>;
  "legacy": boolean;
};

export type ProjectCatalog = {
  "projects": Array<Project>;
  "selected_project_id": string | null;
  "direct_store": boolean;
  "storage_scope": string | null;
};

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
  [key: string]: unknown;
};

export type Summary = {
  "sample_count": number;
  "total_weight": number | null;
  "weight_unit": string;
  "span_count": number;
  "metric_count": number;
  "relation_count": number;
  "diagnostics": Array<string>;
  [key: string]: unknown;
};

export type RunPage = {
  "runs": Array<Run>;
  "next_cursor": string | null;
  [key: string]: unknown;
};

export type RunResult = {
  "run": Run;
  [key: string]: unknown;
};

export type RunDetails = {
  "run": Run;
  "summary": Summary;
  "result_identity"?: string;
  [key: string]: unknown;
};

export type ImportResult = {
  "run": Run;
  "accepted": boolean;
  "summary": MutationSummary;
  "summary_diagnostic"?: string;
  [key: string]: unknown;
};

export type ReanalysisResult = {
  "accepted": boolean;
  "diagnostics": Array<string>;
  "summary": MutationSummary;
  "summary_diagnostic"?: string;
  [key: string]: unknown;
};

export type OperationError = {
  "code": string;
  "message": string;
  "details"?: unknown;
  [key: string]: unknown;
};

export type HttpSuccess = {
  "result": unknown;
  [key: string]: unknown;
};

export type HttpFailure = {
  "error": OperationError;
  [key: string]: unknown;
};

export type McpToolSuccess = {
  "content": Array<{
  "type": "text";
  "text": string;
  [key: string]: unknown;
}>;
  "structuredContent": unknown;
  "isError": false;
  [key: string]: unknown;
};

export type McpToolFailure = {
  "content": Array<{
  "type": "text";
  "text": string;
  [key: string]: unknown;
}>;
  "isError": true;
  "_meta"?: {
  "hunin/operation-error"?: OperationFailure;
  [key: string]: unknown;
};
  [key: string]: unknown;
};

export type CapabilityMode = {
  "mode": string;
  "mechanism": string;
  "availability": string;
  "basis": string;
  "recording_verified": boolean;
  "launch_runtime"?: string;
  "command_requirement"?: string;
  "default_executable"?: string | null;
  "selectable"?: boolean;
  [key: string]: unknown;
};

export type Capability = {
  "runtime": string;
  "available": boolean;
  "executable"?: string | null;
  "version"?: string | null;
  "probe_status": string;
  "capabilities"?: Array<string>;
  "modes"?: Array<CapabilityMode>;
  "diagnostics": Array<string>;
  "verification"?: string;
  [key: string]: unknown;
};

export type NativeSamplingPermission = {
  "state": string;
  "probe"?: string;
  "error"?: string;
  "perf_event_paranoid"?: string | null;
  "prerequisite"?: string;
  "setup_url"?: string;
  "xperf_executable"?: string;
  [key: string]: unknown;
};

export type Capabilities = {
  "host": {
  "os": string;
  "arch": string;
  [key: string]: unknown;
};
  "runtimes": Array<Capability>;
  "schema_version": 1;
  "native_sampling_permission": NativeSamplingPermission;
  "probe_limits": {
  "timeout_seconds_per_command": number;
  "output_bytes_per_stream": number;
  [key: string]: unknown;
};
  "verification": string;
  "import_formats"?: Array<ImportFormat>;
  [key: string]: unknown;
};

export type SourceRead = {
  "path": string;
  "content": string;
  "start_line": number;
  "identity_verified": boolean;
  "source_kind": string;
  "build_mapping_verified": boolean;
  "diagnostic": string;
  "provenance"?: unknown;
  "next_line"?: number | null;
  "total_lines"?: number;
  [key: string]: unknown;
};

export type AnalysisPage = {
  "analyses": Array<unknown>;
  "next_cursor": string | null;
  [key: string]: unknown;
};

export type AnalysisResultDocument = {
  "selected": boolean;
  [key: string]: unknown;
};

export type AnalysisResultPage = {
  "results": Array<AnalysisResultDocument>;
  "next_cursor": string | null;
  [key: string]: unknown;
};

export type AnalysisPackage = {
  "id": string;
  "name": string;
  "version": string;
  "command": Array<string>;
  "requirements": Record<string, unknown>;
  "dependencies": Record<string, unknown>;
  "package_digest": string;
  "files": Record<string, unknown>;
  "package_dir": string;
  "created_at_us": number;
  "executable": {
  "path": string;
  "sha256": string;
  "packaged": boolean;
  [key: string]: unknown;
};
  "dependency_state": string;
  [key: string]: unknown;
};

export type AnalysisRegistered = {
  "analysis": AnalysisPackage;
  [key: string]: unknown;
};

export type AnalysisStarted = {
  "result_set_id": string;
  "status": string;
  "diagnostics": Array<string>;
  [key: string]: unknown;
};

export type AnalysisCanceled = {
  "execution_id": string;
  "status": unknown;
  "diagnostics": Array<string>;
  [key: string]: unknown;
};

export type ScenarioPage = {
  "scenarios": Array<unknown>;
  "next_cursor": string | null;
  [key: string]: unknown;
};

export type ScenarioSaved = {
  "id": string;
  "name": string;
  "command": Array<string>;
  "cwd"?: string;
  "runtime"?: string;
  "timeout_seconds"?: number;
  "expected_exit_code"?: number;
  "env"?: Record<string, string>;
  "input_files"?: Array<string>;
  "expected_stdout"?: string;
  "expected_stdout_sha256"?: string;
  [key: string]: unknown;
};

export type ScenarioSaveResult = {
  "scenario": ScenarioSaved;
  [key: string]: unknown;
};

export type ContextObservation = (unknown & Observation & {
  "kind": "context";
  [key: string]: unknown;
});

export type RelationObservation = (unknown & Observation & {
  "kind": "relation";
  [key: string]: unknown;
});

export type Hotspot = {
  "function": string;
  "file": string | null;
  "line": number | null;
  "module": string | null;
  "code_id": string | null;
  "source_position_basis": string;
  "self_weight": number | null;
  "inclusive_weight": number | null;
  "weight_unit": string;
  [key: string]: unknown;
};

export type ProfileHotspots = {
  "hotspots": Array<Hotspot>;
  "total_weight": number | null;
  "weight_unit": string;
  [key: string]: unknown;
};

export type TreeNode = {
  "id": string;
  "name": string;
  "file": string | null;
  "line": number | null;
  "module": string | null;
  "code_id": string | null;
  "source_position_basis": string;
  "self_weight": number | null;
  "total_weight": number | null;
  "child_count": number;
  "children_truncated": boolean;
  "depth_truncated": boolean;
  "children": Array<TreeNode>;
  [key: string]: unknown;
};

export type ProfileTree = {
  "roots": Array<TreeNode>;
  "weight_unit": string;
  "total_weight": number | null;
  "scope_self_weight": number | null;
  "path": Array<string>;
  "returned_nodes": number;
  "root_count": number;
  "next_cursor": string | null;
  "node_limit": number;
  "depth_limit": number;
  "diagnostics": Array<string>;
  [key: string]: unknown;
};

export type Timeline = {
  "records": Array<Observation>;
  "unaligned_records": Array<Observation>;
  "alignment": string;
  "next_cursor": string | null;
  "bounds": {
  "start_us": number;
  "end_us": number;
  [key: string]: unknown;
} | null;
  "unaligned_count": number;
  "unaligned_clock_domains": Array<{
  "producer_id": string;
  "clock": unknown;
  [key: string]: unknown;
}>;
  "total_count": number;
  "returned_count": number;
  [key: string]: unknown;
};

export type ResourcePoint = {
  "id": string;
  "timestamp_us": number;
  "value": number;
  "pid": number;
  "tid": string;
  "context_id": string | null;
  "producer_id": string;
  [key: string]: unknown;
};

export type ResourceSeries = {
  "id": string;
  "name": string;
  "unit": string;
  "scope": string;
  "pid": number;
  "tid": string;
  "producer_id": string;
  "context_id": string | null;
  "thread_id_source": string;
  "clock": string;
  "clock_alignment": string;
  "time_mapping": string;
  "process_started_at_seconds": unknown;
  "process_lifetime_id": unknown;
  "process_role": unknown;
  "temporality": unknown;
  "point_count": number;
  "point_start": number;
  "returned_points": number;
  "truncated": boolean;
  "points": Array<ResourcePoint>;
  [key: string]: unknown;
};

export type Resources = {
  "series": Array<ResourceSeries>;
  "next_cursor": string | null;
  "returned_points": number;
  "total_points": number;
  "point_limit": number;
  "unaligned_point_count": number;
  "diagnostics": Array<string>;
  [key: string]: unknown;
};

export type Contexts = {
  "contexts": Array<ContextObservation>;
  "relations": Array<RelationObservation>;
  "next_cursor": string | null;
  [key: string]: unknown;
};

export type Evidence = {
  "records": Array<Observation>;
  [key: string]: unknown;
};

export type ComparisonMetric = {
  "name": string;
  "unit": string;
  "scope": string;
  "basis": string;
  "before": number | null;
  "after": number | null;
  "delta": number | null;
  "percent_change": number | null;
  "measurement_quality"?: string;
  "comparable"?: boolean;
  "before_observations"?: number;
  "after_observations"?: number;
  "before_process_lifetimes"?: number;
  "after_process_lifetimes"?: number;
  "diagnostic"?: string;
  "values_available"?: boolean;
  "measurement_comparable"?: boolean;
  "workload_comparable"?: boolean;
  [key: string]: unknown;
};

export type ComparisonHotspot = {
  "function": string;
  "file": string | null;
  "module": string | null;
  "before": number | null;
  "after": number | null;
  "delta": number | null;
  "unit": string;
  "basis": string;
  "before_available": boolean;
  "after_available": boolean;
  "presence"?: "both_observed" | "before_only_observed" | "after_only_observed" | "unknown";
  [key: string]: unknown;
};

export type Comparison = {
  "before": Run;
  "after": Run;
  "comparable": boolean;
  "reasons": Array<string>;
  "metrics": Array<ComparisonMetric>;
  "correctness": {
  "before": unknown;
  "after": unknown;
  [key: string]: unknown;
};
  "hotspots": Array<ComparisonHotspot>;
  "sample_weight_units": {
  "before": Array<string>;
  "after": Array<string>;
  [key: string]: unknown;
};
  "limitations": Array<string>;
  "hotspot_selection"?: ComparisonHotspotSelection;
  [key: string]: unknown;
};

export type OperationFailure = {
  "code": "unknown_operation" | "invalid_parameters" | "conflict" | "operation_failed" | "capacity_exceeded" | "request_abandoned" | "request_deadline_exceeded" | "work_budget_exceeded";
  "operation": string;
  "message": string;
  "capacity"?: {
  "scope": string;
  "maximum_in_flight": number;
};
  "budget"?: {
  "scope": string;
  "dimension": string;
  "limit": number;
  "observed": number;
};
};

export type ImportFormat = {
  "format": string;
  "label": string;
  "description": string;
  "aliases": Array<string>;
  "requires": Array<string>;
  "input_kind"?: "profile" | "observations";
  "requires_existing_recording": boolean;
};

export type ComparisonHotspotSelection = {
  "weight_unit": string;
  "limit_per_recording": number;
  "before_available": boolean;
  "after_available": boolean;
  "before_total_functions": number;
  "after_total_functions": number;
  "before_truncated": boolean;
  "after_truncated": boolean;
};

export type InspectionFinding = {
  "sha256"?: string;
  "example_run_id"?: string;
  "example_artifact_id"?: string;
  "relative_path"?: string;
  "reason"?: string;
  "kind": "missing_referenced_blob" | "unreferenced_at_snapshot" | "pending_ownership_unknown" | "workspace_ownership_unknown" | "unfollowed_path" | "path_unavailable" | "invalid_artifact_reference";
  "path_kind"?: "file" | "directory" | "symlink" | "other";
  "metadata_reference"?: "present" | "absent" | "unknown";
  "size_bytes"?: string | null;
  "row_ordinal"?: number;
};

export type InspectionScan = {
  "area": "artifact_references" | "artifact_files" | "captures" | "analyzer_executions" | "analyzer_packages";
  "examined": number;
  "status": "not_scanned" | "exhausted" | "limited" | "unavailable";
};

export type StorageInspection = {
  "findings": Array<InspectionFinding>;
  "scans": Array<InspectionScan>;
  "limits_hit": Array<"artifact_rows" | "filesystem_entries" | "findings" | "time">;
  "budgets": {
  "artifact_row_limit": number;
  "filesystem_entry_limit": number;
  "finding_limit": number;
  "budget_ms": number;
};
  "elapsed_ms": number;
  "database_changed_during_scan": boolean;
  "content_hashes_checked": 0;
  "deletion_authorized": false;
  "reference_view": "sqlite_read_transaction";
  "filesystem_view": "live_metadata";
  "path_encoding": "lossy_display_only";
  "limitations": Array<string>;
};

export type RunMetadata = {
  "run": Run;
  "result_identity"?: string;
  "result_identity_diagnostic"?: string;
};

export type RunRemoval = {
  "receipt": Record<string, unknown> | null;
};

export type MutationSummary = {
  "sample_count": number;
  "total_weight": number | null;
  "weight_unit": string;
  "span_count": number;
  "metric_count": number;
  "relation_count": number;
  "diagnostics": Array<string>;
  [key: string]: unknown;
} | null;

export type CountSelector = {
  "name": string;
  "file"?: string;
  "line"?: number;
};

export type CountPlan = {
  "status": "ready" | "blocked";
  "method": string | null;
  "reason": string | null;
  "requested_scope": "all" | "selected";
  "collection_scope": "all" | "selected" | "all_then_filter";
  "functions": Array<CountSelector>;
  "requires_rebuild": boolean;
  "limitations": Array<string>;
};

export type CountPlanResult = {
  "plan": CountPlan;
};

export type CountMeasurement = {
  "run"?: Run;
  "plan": CountPlan;
  "parent_run_id": string;
  "reused": boolean;
};

export type CountFunction = {
  "code_id"?: string;
  "name": string;
  "file": string | null;
  "line": number | null;
};

export type FunctionCount = {
  "function": CountFunction;
  "calls": number;
  "resumes": number | null;
};

export type CallCounts = {
  "status": "not_collected" | "running" | "complete" | "incomplete" | "failed" | "canceled";
  "run_id": string;
  "parent_run_id": string | null;
  "method": string | null;
  "requested_scope": "all" | "selected";
  "collection_scope": "all" | "selected" | "all_then_filter";
  "counts": Array<FunctionCount>;
  "next_cursor": string | null;
  "total_count": number;
  "diagnostics": Array<string>;
  "limitations": Array<string>;
};

export type ProfileInspectionFunction = {
  "id": string;
  "function": string;
  "file": string | null;
  "line": number | null;
  "module": string | null;
  "code_id": string | null;
  "source_position_basis": string;
  "self_weight": number | null;
  "inclusive_weight": number | null;
  "weight_unit": string;
  [key: string]: unknown;
};

export type ProfileInspectionCaller = {
  "id": string;
  "function": string;
  "file": string | null;
  "line": number | null;
  "module": string | null;
  "code_id": string | null;
  "weight": number | null;
  "sample_count": number;
  [key: string]: unknown;
};

export type ProfileInspectionNode = {
  "id": string;
  "name": string;
  "file": string | null;
  "line": number | null;
  "module": string | null;
  "code_id": string | null;
  "source_position_basis": string;
  "self_weight": number | null;
  "total_weight": number | null;
  "child_count": number;
  "children_truncated": boolean;
  "depth_truncated": boolean;
  "children": Array<ProfileInspectionNode>;
  "frame_id": string;
  [key: string]: unknown;
};

export type ProfileInspectionSelection = {
  "id": string;
  "function": string;
  "file": string | null;
  "line": number | null;
  "module": string | null;
  "code_id": string | null;
  "source_position_basis": string;
  "self_weight": number | null;
  "inclusive_weight": number | null;
  "weight_unit": string;
  "sample_count": number;
  "observed_start_us": number;
  "observed_end_us": number;
  "callers": Array<ProfileInspectionCaller>;
  "evidence_ids": Array<string>;
  "evidence_count": number;
  "evidence_truncated": boolean;
  "callers_truncated": boolean;
  [key: string]: unknown;
} | null;

export type ProfileInspectionBin = {
  "start_us": number;
  "end_us": number;
  "weight": number | null;
  "selected_weight": number | null;
  "sample_count": number;
  [key: string]: unknown;
};

export type ProfileInspection = {
  "result_identity": string;
  "weight_unit": string;
  "units": Array<string>;
  "bounds": {
  "start_us": number;
  "end_us": number;
  [key: string]: unknown;
} | null;
  "window": {
  "start_us": number;
  "end_us": number;
  [key: string]: unknown;
} | null;
  "sample_count": number;
  "total_sample_count": number;
  "total_weight": number | null;
  "capture_weight": number | null;
  "unaligned_count": number;
  "overview": Array<ProfileInspectionBin>;
  "activity": Array<ProfileInspectionBin>;
  "functions": Array<ProfileInspectionFunction>;
  "total_functions": number;
  "functions_truncated": boolean;
  "roots": Array<ProfileInspectionNode>;
  "tree_truncated": boolean;
  "selected": ProfileInspectionSelection;
  "diagnostics": Array<string>;
  [key: string]: unknown;
};

export type OperationInputs = {
  "capabilities": {

};
  "storage.inspect": {
  "artifact_row_limit"?: number;
  "filesystem_entry_limit"?: number;
  "finding_limit"?: number;
  "budget_ms"?: number;
};
  "runs.list": {
  "limit"?: number;
  "cursor"?: string;
};
  "runs.metadata": {
  "run_id": string;
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
  "env"?: Record<string, string>;
  "input_files"?: Array<string>;
  "expected_stdout"?: string;
  "expected_stdout_sha256"?: string;
  "call_counting"?: {
  "functions"?: Array<{
  "name": string;
  "file"?: string;
  "line"?: number;
}>;
  "allow_rebuild"?: boolean;
  "build_command"?: Array<string>;
  "max_functions"?: number;
  "max_output_bytes"?: number;
};
};
  "runs.cancel": {
  "run_id": string;
};
  "runs.remove": {
  "run_id": string;
};
  "runs.removal": {
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
  "profiles.inspect": {
  "run_id": string;
  "weight_unit"?: string;
  "start_us"?: number;
  "end_us"?: number;
  "bins"?: number;
  "function_limit"?: number;
  "node_limit"?: number;
  "depth_limit"?: number;
  "frame_id"?: string;
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
  "weight_unit"?: string;
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
  "requirements": {
  "runtime": string;
  "source_fingerprint"?: string;
  "source_independent"?: boolean;
  "artifact_digests"?: Array<string>;
  "executable_sha256"?: string;
  "runtime_version"?: string;
};
  "version"?: string;
  "files"?: Array<string>;
  "dependencies"?: {
  "python_requirements"?: string;
  "node_lockfile"?: string;
  "setup_commands"?: Array<Array<string>>;
};
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
  "execution_id"?: string;
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
  "env"?: Record<string, string>;
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
  "calls.plan": {
  "run_id": string;
  "functions"?: Array<{
  "name": string;
  "file"?: string;
  "line"?: number;
}>;
  "allow_rebuild"?: boolean;
  "build_command"?: Array<string>;
  "max_functions"?: number;
  "max_output_bytes"?: number;
};
  "calls.measure": {
  "run_id": string;
  "functions"?: Array<{
  "name": string;
  "file"?: string;
  "line"?: number;
}>;
  "allow_rebuild"?: boolean;
  "build_command"?: Array<string>;
  "max_functions"?: number;
  "max_output_bytes"?: number;
  "timeout_seconds"?: number;
  "request_id"?: string;
};
  "calls.query": {
  "run_id": string;
  "limit"?: number;
  "cursor"?: string;
};
};

export type OperationOutputs = {
  "capabilities": Capabilities;
  "storage.inspect": StorageInspection;
  "runs.list": RunPage;
  "runs.metadata": RunMetadata;
  "runs.get": RunDetails;
  "runs.start": RunResult;
  "runs.cancel": RunResult;
  "runs.remove": {
  "removed": true;
  [key: string]: unknown;
};
  "runs.removal": RunRemoval;
  "profiles.hotspots": ProfileHotspots;
  "profiles.tree": ProfileTree;
  "profiles.inspect": ProfileInspection;
  "timeline.query": Timeline;
  "resources.query": Resources;
  "contexts.query": Contexts;
  "evidence.get": Evidence;
  "artifacts.list": {
  "artifacts": Array<{
  "id": string;
  "name": string;
  "size": number;
  "sha256": string;
  "format": string;
  [key: string]: unknown;
}>;
  "next_cursor": string | null;
  [key: string]: unknown;
};
  "artifacts.read": {
  "content": string;
  "encoding": "utf8_lossy";
  "next_offset": number | null;
  "size": number;
  [key: string]: unknown;
};
  "source.read": SourceRead;
  "recordings.import": ImportResult;
  "comparisons.create": Comparison;
  "analyses.list": AnalysisPage;
  "analyses.register": AnalysisRegistered;
  "analyses.run": AnalysisStarted;
  "analyses.results": AnalysisResultPage;
  "scenarios.list": ScenarioPage;
  "scenarios.save": ScenarioSaveResult;
  "scenarios.run": RunResult;
  "analyses.cancel": AnalysisCanceled;
  "recordings.reanalyze": ReanalysisResult;
  "calls.plan": CountPlanResult;
  "calls.measure": CountMeasurement;
  "calls.query": CallCounts;
};

function isOutputObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function isProject(value: unknown): value is Project {
  return (isOutputObject(value) && Object.hasOwn(value, "id") && Object.hasOwn(value, "name") && Object.hasOwn(value, "store_path") && Object.hasOwn(value, "workspace_roots") && Object.hasOwn(value, "legacy") && (typeof value["id"] === "string" && [...value["id"]].length >= 1) && (typeof value["name"] === "string" && [...value["name"]].length >= 1) && (typeof value["store_path"] === "string" && [...value["store_path"]].length >= 1) && (Array.isArray(value["workspace_roots"]) && value["workspace_roots"].every(item => (typeof item === "string" && [...item].length >= 1))) && (typeof value["legacy"] === "boolean") && Object.keys(value).every(key => ["id","name","store_path","workspace_roots","legacy"].includes(key)));
}

export function isProjectCatalog(value: unknown): value is ProjectCatalog {
  return (isOutputObject(value) && Object.hasOwn(value, "projects") && Object.hasOwn(value, "selected_project_id") && Object.hasOwn(value, "direct_store") && Object.hasOwn(value, "storage_scope") && (Array.isArray(value["projects"]) && value["projects"].every(item => isProject(item))) && ((typeof value["selected_project_id"] === "string") || (value["selected_project_id"] === null)) && (typeof value["direct_store"] === "boolean") && ((typeof value["storage_scope"] === "string") || (value["storage_scope"] === null)) && Object.keys(value).every(key => ["projects","selected_project_id","direct_store","storage_scope"].includes(key)));
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
  return (isOutputObject(value) && Object.hasOwn(value, "run") && Object.hasOwn(value, "summary") && isRun(value["run"]) && isSummary(value["summary"]) && (!Object.hasOwn(value, "result_identity") || (typeof value["result_identity"] === "string")));
}

export function isImportResult(value: unknown): value is ImportResult {
  return (isOutputObject(value) && Object.hasOwn(value, "run") && Object.hasOwn(value, "accepted") && Object.hasOwn(value, "summary") && isRun(value["run"]) && (typeof value["accepted"] === "boolean") && isMutationSummary(value["summary"]) && (!Object.hasOwn(value, "summary_diagnostic") || (typeof value["summary_diagnostic"] === "string")));
}

export function isReanalysisResult(value: unknown): value is ReanalysisResult {
  return (isOutputObject(value) && Object.hasOwn(value, "accepted") && Object.hasOwn(value, "diagnostics") && Object.hasOwn(value, "summary") && (typeof value["accepted"] === "boolean") && (Array.isArray(value["diagnostics"]) && value["diagnostics"].every(item => (typeof item === "string"))) && isMutationSummary(value["summary"]) && (!Object.hasOwn(value, "summary_diagnostic") || (typeof value["summary_diagnostic"] === "string")));
}

export function isOperationError(value: unknown): value is OperationError {
  return (isOutputObject(value) && Object.hasOwn(value, "code") && Object.hasOwn(value, "message") && (typeof value["code"] === "string") && (typeof value["message"] === "string") && (!Object.hasOwn(value, "details") || ((!isOutputObject(value["details"]) || (isOutputObject(value["details"]) && (!Object.hasOwn(value["details"], "operation_error") || isOperationFailure(value["details"]["operation_error"])))))));
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
  return (isOutputObject(value) && Object.hasOwn(value, "content") && Object.hasOwn(value, "isError") && (Array.isArray(value["content"]) && value["content"].every(item => (isOutputObject(item) && Object.hasOwn(item, "type") && Object.hasOwn(item, "text") && (item["type"] === "text") && (typeof item["text"] === "string")))) && (value["isError"] === true) && (!Object.hasOwn(value, "_meta") || (isOutputObject(value["_meta"]) && (!Object.hasOwn(value["_meta"], "hunin/operation-error") || isOperationFailure(value["_meta"]["hunin/operation-error"])))));
}

export function isCapabilityMode(value: unknown): value is CapabilityMode {
  return (isOutputObject(value) && Object.hasOwn(value, "mode") && Object.hasOwn(value, "mechanism") && Object.hasOwn(value, "availability") && Object.hasOwn(value, "basis") && Object.hasOwn(value, "recording_verified") && (typeof value["mode"] === "string") && (typeof value["mechanism"] === "string") && (typeof value["availability"] === "string") && (typeof value["basis"] === "string") && (typeof value["recording_verified"] === "boolean") && (!Object.hasOwn(value, "launch_runtime") || (typeof value["launch_runtime"] === "string")) && (!Object.hasOwn(value, "command_requirement") || (typeof value["command_requirement"] === "string")) && (!Object.hasOwn(value, "default_executable") || ((typeof value["default_executable"] === "string") || (value["default_executable"] === null))) && (!Object.hasOwn(value, "selectable") || (typeof value["selectable"] === "boolean")));
}

export function isCapability(value: unknown): value is Capability {
  return (isOutputObject(value) && Object.hasOwn(value, "runtime") && Object.hasOwn(value, "available") && Object.hasOwn(value, "probe_status") && Object.hasOwn(value, "diagnostics") && (typeof value["runtime"] === "string") && (typeof value["available"] === "boolean") && (!Object.hasOwn(value, "executable") || ((typeof value["executable"] === "string") || (value["executable"] === null))) && (!Object.hasOwn(value, "version") || ((typeof value["version"] === "string") || (value["version"] === null))) && (typeof value["probe_status"] === "string") && (!Object.hasOwn(value, "capabilities") || (Array.isArray(value["capabilities"]) && value["capabilities"].every(item => (typeof item === "string")))) && (!Object.hasOwn(value, "modes") || (Array.isArray(value["modes"]) && value["modes"].every(item => isCapabilityMode(item)))) && (Array.isArray(value["diagnostics"]) && value["diagnostics"].every(item => (typeof item === "string"))) && (!Object.hasOwn(value, "verification") || (typeof value["verification"] === "string")));
}

export function isNativeSamplingPermission(value: unknown): value is NativeSamplingPermission {
  return (isOutputObject(value) && Object.hasOwn(value, "state") && (typeof value["state"] === "string") && (!Object.hasOwn(value, "probe") || (typeof value["probe"] === "string")) && (!Object.hasOwn(value, "error") || (typeof value["error"] === "string")) && (!Object.hasOwn(value, "perf_event_paranoid") || ((typeof value["perf_event_paranoid"] === "string") || (value["perf_event_paranoid"] === null))) && (!Object.hasOwn(value, "prerequisite") || (typeof value["prerequisite"] === "string")) && (!Object.hasOwn(value, "setup_url") || (typeof value["setup_url"] === "string")) && (!Object.hasOwn(value, "xperf_executable") || (typeof value["xperf_executable"] === "string")));
}

export function isCapabilities(value: unknown): value is Capabilities {
  return (isOutputObject(value) && Object.hasOwn(value, "host") && Object.hasOwn(value, "runtimes") && Object.hasOwn(value, "schema_version") && Object.hasOwn(value, "native_sampling_permission") && Object.hasOwn(value, "probe_limits") && Object.hasOwn(value, "verification") && (isOutputObject(value["host"]) && Object.hasOwn(value["host"], "os") && Object.hasOwn(value["host"], "arch") && (typeof value["host"]["os"] === "string") && (typeof value["host"]["arch"] === "string")) && (Array.isArray(value["runtimes"]) && value["runtimes"].every(item => isCapability(item))) && (value["schema_version"] === 1) && isNativeSamplingPermission(value["native_sampling_permission"]) && (isOutputObject(value["probe_limits"]) && Object.hasOwn(value["probe_limits"], "timeout_seconds_per_command") && Object.hasOwn(value["probe_limits"], "output_bytes_per_stream") && (typeof value["probe_limits"]["timeout_seconds_per_command"] === "number" && Number.isInteger(value["probe_limits"]["timeout_seconds_per_command"]) && value["probe_limits"]["timeout_seconds_per_command"] >= 0) && (typeof value["probe_limits"]["output_bytes_per_stream"] === "number" && Number.isInteger(value["probe_limits"]["output_bytes_per_stream"]) && value["probe_limits"]["output_bytes_per_stream"] >= 0)) && (typeof value["verification"] === "string") && (!Object.hasOwn(value, "import_formats") || (Array.isArray(value["import_formats"]) && value["import_formats"].every(item => isImportFormat(item)))));
}

export function isSourceRead(value: unknown): value is SourceRead {
  return (isOutputObject(value) && Object.hasOwn(value, "path") && Object.hasOwn(value, "content") && Object.hasOwn(value, "start_line") && Object.hasOwn(value, "identity_verified") && Object.hasOwn(value, "source_kind") && Object.hasOwn(value, "build_mapping_verified") && Object.hasOwn(value, "diagnostic") && (typeof value["path"] === "string") && (typeof value["content"] === "string") && (typeof value["start_line"] === "number" && Number.isInteger(value["start_line"]) && value["start_line"] >= 1) && (typeof value["identity_verified"] === "boolean") && (typeof value["source_kind"] === "string") && (typeof value["build_mapping_verified"] === "boolean") && (typeof value["diagnostic"] === "string") && (!Object.hasOwn(value, "provenance") || true) && (!Object.hasOwn(value, "next_line") || ((typeof value["next_line"] === "number" && Number.isInteger(value["next_line"]) && value["next_line"] >= 1) || (value["next_line"] === null))) && (!Object.hasOwn(value, "total_lines") || (typeof value["total_lines"] === "number" && Number.isInteger(value["total_lines"]) && value["total_lines"] >= 0)));
}

export function isAnalysisPage(value: unknown): value is AnalysisPage {
  return (isOutputObject(value) && Object.hasOwn(value, "analyses") && Object.hasOwn(value, "next_cursor") && (Array.isArray(value["analyses"])) && ((typeof value["next_cursor"] === "string") || (value["next_cursor"] === null)));
}

export function isAnalysisResultDocument(value: unknown): value is AnalysisResultDocument {
  return (isOutputObject(value) && Object.hasOwn(value, "selected") && (typeof value["selected"] === "boolean"));
}

export function isAnalysisResultPage(value: unknown): value is AnalysisResultPage {
  return (isOutputObject(value) && Object.hasOwn(value, "results") && Object.hasOwn(value, "next_cursor") && (Array.isArray(value["results"]) && value["results"].every(item => isAnalysisResultDocument(item))) && ((typeof value["next_cursor"] === "string") || (value["next_cursor"] === null)));
}

export function isAnalysisPackage(value: unknown): value is AnalysisPackage {
  return (isOutputObject(value) && Object.hasOwn(value, "id") && Object.hasOwn(value, "name") && Object.hasOwn(value, "version") && Object.hasOwn(value, "command") && Object.hasOwn(value, "requirements") && Object.hasOwn(value, "dependencies") && Object.hasOwn(value, "package_digest") && Object.hasOwn(value, "files") && Object.hasOwn(value, "package_dir") && Object.hasOwn(value, "created_at_us") && Object.hasOwn(value, "executable") && Object.hasOwn(value, "dependency_state") && (typeof value["id"] === "string") && (typeof value["name"] === "string") && (typeof value["version"] === "string") && (Array.isArray(value["command"]) && value["command"].every(item => (typeof item === "string"))) && (isOutputObject(value["requirements"])) && (isOutputObject(value["dependencies"])) && (typeof value["package_digest"] === "string") && (isOutputObject(value["files"])) && (typeof value["package_dir"] === "string") && (typeof value["created_at_us"] === "number" && Number.isInteger(value["created_at_us"]) && value["created_at_us"] >= 0) && (isOutputObject(value["executable"]) && Object.hasOwn(value["executable"], "path") && Object.hasOwn(value["executable"], "sha256") && Object.hasOwn(value["executable"], "packaged") && (typeof value["executable"]["path"] === "string") && (typeof value["executable"]["sha256"] === "string") && (typeof value["executable"]["packaged"] === "boolean")) && (typeof value["dependency_state"] === "string"));
}

export function isAnalysisRegistered(value: unknown): value is AnalysisRegistered {
  return (isOutputObject(value) && Object.hasOwn(value, "analysis") && isAnalysisPackage(value["analysis"]));
}

export function isAnalysisStarted(value: unknown): value is AnalysisStarted {
  return (isOutputObject(value) && Object.hasOwn(value, "result_set_id") && Object.hasOwn(value, "status") && Object.hasOwn(value, "diagnostics") && (typeof value["result_set_id"] === "string") && (typeof value["status"] === "string") && (Array.isArray(value["diagnostics"]) && value["diagnostics"].every(item => (typeof item === "string"))));
}

export function isAnalysisCanceled(value: unknown): value is AnalysisCanceled {
  return (isOutputObject(value) && Object.hasOwn(value, "execution_id") && Object.hasOwn(value, "status") && Object.hasOwn(value, "diagnostics") && (typeof value["execution_id"] === "string") && true && (Array.isArray(value["diagnostics"]) && value["diagnostics"].every(item => (typeof item === "string"))));
}

export function isScenarioPage(value: unknown): value is ScenarioPage {
  return (isOutputObject(value) && Object.hasOwn(value, "scenarios") && Object.hasOwn(value, "next_cursor") && (Array.isArray(value["scenarios"])) && ((typeof value["next_cursor"] === "string") || (value["next_cursor"] === null)));
}

export function isScenarioSaved(value: unknown): value is ScenarioSaved {
  return (isOutputObject(value) && Object.hasOwn(value, "id") && Object.hasOwn(value, "name") && Object.hasOwn(value, "command") && (typeof value["id"] === "string") && (typeof value["name"] === "string") && (Array.isArray(value["command"]) && value["command"].every(item => (typeof item === "string"))) && (!Object.hasOwn(value, "cwd") || (typeof value["cwd"] === "string")) && (!Object.hasOwn(value, "runtime") || (typeof value["runtime"] === "string")) && (!Object.hasOwn(value, "timeout_seconds") || (typeof value["timeout_seconds"] === "number" && Number.isFinite(value["timeout_seconds"]))) && (!Object.hasOwn(value, "expected_exit_code") || (typeof value["expected_exit_code"] === "number" && Number.isInteger(value["expected_exit_code"]))) && (!Object.hasOwn(value, "env") || (isOutputObject(value["env"]) && Object.values(value["env"]).every(item => (typeof item === "string")))) && (!Object.hasOwn(value, "input_files") || (Array.isArray(value["input_files"]) && value["input_files"].every(item => (typeof item === "string")))) && (!Object.hasOwn(value, "expected_stdout") || (typeof value["expected_stdout"] === "string")) && (!Object.hasOwn(value, "expected_stdout_sha256") || (typeof value["expected_stdout_sha256"] === "string")));
}

export function isScenarioSaveResult(value: unknown): value is ScenarioSaveResult {
  return (isOutputObject(value) && Object.hasOwn(value, "scenario") && isScenarioSaved(value["scenario"]));
}

export function isObservation(value: unknown): value is Observation {
  return (((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "sample"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "stack") && Object.hasOwn(value, "weight") && Object.hasOwn(value, "weight_unit")))) : true)) && ((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "span"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "name") && Object.hasOwn(value, "duration_us")))) : true)) && ((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "event"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "name")))) : true)) && ((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "metric"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "name") && Object.hasOwn(value, "value") && Object.hasOwn(value, "unit") && Object.hasOwn(value, "scope")))) : true)) && ((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "relation"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "from") && Object.hasOwn(value, "to") && Object.hasOwn(value, "relation") && Object.hasOwn(value, "evidence")))) : true)) && ((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "context"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "name")))) : true)) && isOutputObject(value) && Object.hasOwn(value, "schema_version") && Object.hasOwn(value, "kind") && Object.hasOwn(value, "id") && Object.hasOwn(value, "run_id") && Object.hasOwn(value, "producer_id") && Object.hasOwn(value, "pid") && Object.hasOwn(value, "tid") && Object.hasOwn(value, "timestamp_us") && Object.hasOwn(value, "basis") && (value["schema_version"] === 1) && ((value["kind"] === "sample" || value["kind"] === "span" || value["kind"] === "event" || value["kind"] === "metric" || value["kind"] === "relation" || value["kind"] === "context")) && (typeof value["id"] === "string" && [...value["id"]].length >= 1) && (typeof value["run_id"] === "string" && [...value["run_id"]].length >= 1) && (typeof value["producer_id"] === "string" && [...value["producer_id"]].length >= 1) && (typeof value["pid"] === "number" && Number.isInteger(value["pid"]) && value["pid"] >= 0) && (typeof value["tid"] === "string") && (typeof value["timestamp_us"] === "number" && Number.isInteger(value["timestamp_us"]) && value["timestamp_us"] >= 0 && value["timestamp_us"] <= 9007199254740991) && (!Object.hasOwn(value, "name") || (typeof value["name"] === "string")) && (!Object.hasOwn(value, "duration_us") || (typeof value["duration_us"] === "number" && Number.isInteger(value["duration_us"]) && value["duration_us"] >= 0 && value["duration_us"] <= 9007199254740991)) && (!Object.hasOwn(value, "parent_id") || (typeof value["parent_id"] === "string")) && (!Object.hasOwn(value, "context_id") || (typeof value["context_id"] === "string")) && (!Object.hasOwn(value, "stack") || (Array.isArray(value["stack"]) && value["stack"].every(item => (isOutputObject(item) && Object.hasOwn(item, "function") && (typeof item["function"] === "string") && (!Object.hasOwn(item, "file") || (typeof item["file"] === "string")) && (!Object.hasOwn(item, "line") || (typeof item["line"] === "number" && Number.isInteger(item["line"]) && item["line"] >= 1)) && (!Object.hasOwn(item, "column") || (typeof item["column"] === "number" && Number.isInteger(item["column"]) && item["column"] >= 0)) && (!Object.hasOwn(item, "module") || (typeof item["module"] === "string")) && (!Object.hasOwn(item, "address") || (typeof item["address"] === "string")) && (!Object.hasOwn(item, "code_id") || (typeof item["code_id"] === "string")) && Object.keys(item).every(key => ["function","file","line","column","module","address","code_id"].includes(key)))))) && (!Object.hasOwn(value, "weight") || (typeof value["weight"] === "number" && Number.isFinite(value["weight"]) && value["weight"] > 0)) && (!Object.hasOwn(value, "weight_unit") || ((value["weight_unit"] === "samples" || value["weight_unit"] === "microseconds" || value["weight_unit"] === "bytes" || value["weight_unit"] === "count"))) && (!Object.hasOwn(value, "interval_us") || (typeof value["interval_us"] === "number" && Number.isFinite(value["interval_us"]) && value["interval_us"] > 0)) && (!Object.hasOwn(value, "value") || (typeof value["value"] === "number" && Number.isFinite(value["value"]))) && (!Object.hasOwn(value, "unit") || (typeof value["unit"] === "string")) && (!Object.hasOwn(value, "scope") || ((value["scope"] === "process" || value["scope"] === "thread" || value["scope"] === "context" || value["scope"] === "run"))) && (!Object.hasOwn(value, "from") || (typeof value["from"] === "string")) && (!Object.hasOwn(value, "to") || (typeof value["to"] === "string")) && (!Object.hasOwn(value, "relation") || (typeof value["relation"] === "string")) && (!Object.hasOwn(value, "evidence") || (Array.isArray(value["evidence"]) && new Set(value["evidence"]).size === value["evidence"].length && value["evidence"].every(item => (typeof item === "string")))) && ((value["basis"] === "observed" || value["basis"] === "sampled" || value["basis"] === "explicit" || value["basis"] === "derived")) && (!Object.hasOwn(value, "attributes") || (isOutputObject(value["attributes"]) && (!Object.hasOwn(value["attributes"], "clock") || true) && (!Object.hasOwn(value["attributes"], "clock_alignment") || true))) && Object.keys(value).every(key => ["schema_version","kind","id","run_id","producer_id","pid","tid","timestamp_us","name","duration_us","parent_id","context_id","stack","weight","weight_unit","interval_us","value","unit","scope","from","to","relation","evidence","basis","attributes"].includes(key)));
}

export function isContextObservation(value: unknown): value is ContextObservation {
  return (isObservation(value) && (isOutputObject(value) && Object.hasOwn(value, "kind") && (value["kind"] === "context")));
}

export function isRelationObservation(value: unknown): value is RelationObservation {
  return (isObservation(value) && (isOutputObject(value) && Object.hasOwn(value, "kind") && (value["kind"] === "relation")));
}

export function isHotspot(value: unknown): value is Hotspot {
  return (isOutputObject(value) && Object.hasOwn(value, "function") && Object.hasOwn(value, "file") && Object.hasOwn(value, "line") && Object.hasOwn(value, "module") && Object.hasOwn(value, "code_id") && Object.hasOwn(value, "source_position_basis") && Object.hasOwn(value, "self_weight") && Object.hasOwn(value, "inclusive_weight") && Object.hasOwn(value, "weight_unit") && (typeof value["function"] === "string") && ((typeof value["file"] === "string") || (value["file"] === null)) && ((typeof value["line"] === "number" && Number.isInteger(value["line"])) || (value["line"] === null)) && ((typeof value["module"] === "string") || (value["module"] === null)) && ((typeof value["code_id"] === "string") || (value["code_id"] === null)) && (typeof value["source_position_basis"] === "string") && ((typeof value["self_weight"] === "number" && Number.isFinite(value["self_weight"])) || (value["self_weight"] === null)) && ((typeof value["inclusive_weight"] === "number" && Number.isFinite(value["inclusive_weight"])) || (value["inclusive_weight"] === null)) && (typeof value["weight_unit"] === "string"));
}

export function isProfileHotspots(value: unknown): value is ProfileHotspots {
  return (isOutputObject(value) && Object.hasOwn(value, "hotspots") && Object.hasOwn(value, "total_weight") && Object.hasOwn(value, "weight_unit") && (Array.isArray(value["hotspots"]) && value["hotspots"].every(item => isHotspot(item))) && ((typeof value["total_weight"] === "number" && Number.isFinite(value["total_weight"])) || (value["total_weight"] === null)) && (typeof value["weight_unit"] === "string"));
}

export function isTreeNode(value: unknown): value is TreeNode {
  return (isOutputObject(value) && Object.hasOwn(value, "id") && Object.hasOwn(value, "name") && Object.hasOwn(value, "file") && Object.hasOwn(value, "line") && Object.hasOwn(value, "module") && Object.hasOwn(value, "code_id") && Object.hasOwn(value, "source_position_basis") && Object.hasOwn(value, "self_weight") && Object.hasOwn(value, "total_weight") && Object.hasOwn(value, "child_count") && Object.hasOwn(value, "children_truncated") && Object.hasOwn(value, "depth_truncated") && Object.hasOwn(value, "children") && (typeof value["id"] === "string") && (typeof value["name"] === "string") && ((typeof value["file"] === "string") || (value["file"] === null)) && ((typeof value["line"] === "number" && Number.isInteger(value["line"])) || (value["line"] === null)) && ((typeof value["module"] === "string") || (value["module"] === null)) && ((typeof value["code_id"] === "string") || (value["code_id"] === null)) && (typeof value["source_position_basis"] === "string") && ((typeof value["self_weight"] === "number" && Number.isFinite(value["self_weight"])) || (value["self_weight"] === null)) && ((typeof value["total_weight"] === "number" && Number.isFinite(value["total_weight"])) || (value["total_weight"] === null)) && (typeof value["child_count"] === "number" && Number.isInteger(value["child_count"]) && value["child_count"] >= 0) && (typeof value["children_truncated"] === "boolean") && (typeof value["depth_truncated"] === "boolean") && (Array.isArray(value["children"]) && value["children"].every(item => isTreeNode(item))));
}

export function isProfileTree(value: unknown): value is ProfileTree {
  return (isOutputObject(value) && Object.hasOwn(value, "roots") && Object.hasOwn(value, "weight_unit") && Object.hasOwn(value, "total_weight") && Object.hasOwn(value, "scope_self_weight") && Object.hasOwn(value, "path") && Object.hasOwn(value, "returned_nodes") && Object.hasOwn(value, "root_count") && Object.hasOwn(value, "next_cursor") && Object.hasOwn(value, "node_limit") && Object.hasOwn(value, "depth_limit") && Object.hasOwn(value, "diagnostics") && (Array.isArray(value["roots"]) && value["roots"].every(item => isTreeNode(item))) && (typeof value["weight_unit"] === "string") && ((typeof value["total_weight"] === "number" && Number.isFinite(value["total_weight"])) || (value["total_weight"] === null)) && ((typeof value["scope_self_weight"] === "number" && Number.isFinite(value["scope_self_weight"])) || (value["scope_self_weight"] === null)) && (Array.isArray(value["path"]) && value["path"].every(item => (typeof item === "string"))) && (typeof value["returned_nodes"] === "number" && Number.isInteger(value["returned_nodes"]) && value["returned_nodes"] >= 0) && (typeof value["root_count"] === "number" && Number.isInteger(value["root_count"]) && value["root_count"] >= 0) && ((typeof value["next_cursor"] === "string") || (value["next_cursor"] === null)) && (typeof value["node_limit"] === "number" && Number.isInteger(value["node_limit"]) && value["node_limit"] >= 0) && (typeof value["depth_limit"] === "number" && Number.isInteger(value["depth_limit"]) && value["depth_limit"] >= 0) && (Array.isArray(value["diagnostics"]) && value["diagnostics"].every(item => (typeof item === "string"))));
}

export function isTimeline(value: unknown): value is Timeline {
  return (isOutputObject(value) && Object.hasOwn(value, "records") && Object.hasOwn(value, "unaligned_records") && Object.hasOwn(value, "alignment") && Object.hasOwn(value, "next_cursor") && Object.hasOwn(value, "bounds") && Object.hasOwn(value, "unaligned_count") && Object.hasOwn(value, "unaligned_clock_domains") && Object.hasOwn(value, "total_count") && Object.hasOwn(value, "returned_count") && (Array.isArray(value["records"]) && value["records"].every(item => isObservation(item))) && (Array.isArray(value["unaligned_records"]) && value["unaligned_records"].every(item => isObservation(item))) && (typeof value["alignment"] === "string") && ((typeof value["next_cursor"] === "string") || (value["next_cursor"] === null)) && ((isOutputObject(value["bounds"]) && Object.hasOwn(value["bounds"], "start_us") && Object.hasOwn(value["bounds"], "end_us") && (typeof value["bounds"]["start_us"] === "number" && Number.isInteger(value["bounds"]["start_us"]) && value["bounds"]["start_us"] >= 0) && (typeof value["bounds"]["end_us"] === "number" && Number.isInteger(value["bounds"]["end_us"]) && value["bounds"]["end_us"] >= 0)) || (value["bounds"] === null)) && (typeof value["unaligned_count"] === "number" && Number.isInteger(value["unaligned_count"]) && value["unaligned_count"] >= 0) && (Array.isArray(value["unaligned_clock_domains"]) && value["unaligned_clock_domains"].every(item => (isOutputObject(item) && Object.hasOwn(item, "producer_id") && Object.hasOwn(item, "clock") && (typeof item["producer_id"] === "string") && true))) && (typeof value["total_count"] === "number" && Number.isInteger(value["total_count"]) && value["total_count"] >= 0) && (typeof value["returned_count"] === "number" && Number.isInteger(value["returned_count"]) && value["returned_count"] >= 0));
}

export function isResourcePoint(value: unknown): value is ResourcePoint {
  return (isOutputObject(value) && Object.hasOwn(value, "id") && Object.hasOwn(value, "timestamp_us") && Object.hasOwn(value, "value") && Object.hasOwn(value, "pid") && Object.hasOwn(value, "tid") && Object.hasOwn(value, "context_id") && Object.hasOwn(value, "producer_id") && (typeof value["id"] === "string") && (typeof value["timestamp_us"] === "number" && Number.isInteger(value["timestamp_us"]) && value["timestamp_us"] >= 0) && (typeof value["value"] === "number" && Number.isFinite(value["value"])) && (typeof value["pid"] === "number" && Number.isInteger(value["pid"]) && value["pid"] >= 0) && (typeof value["tid"] === "string") && ((typeof value["context_id"] === "string") || (value["context_id"] === null)) && (typeof value["producer_id"] === "string"));
}

export function isResourceSeries(value: unknown): value is ResourceSeries {
  return (isOutputObject(value) && Object.hasOwn(value, "id") && Object.hasOwn(value, "name") && Object.hasOwn(value, "unit") && Object.hasOwn(value, "scope") && Object.hasOwn(value, "pid") && Object.hasOwn(value, "tid") && Object.hasOwn(value, "producer_id") && Object.hasOwn(value, "context_id") && Object.hasOwn(value, "thread_id_source") && Object.hasOwn(value, "clock") && Object.hasOwn(value, "clock_alignment") && Object.hasOwn(value, "time_mapping") && Object.hasOwn(value, "process_started_at_seconds") && Object.hasOwn(value, "process_lifetime_id") && Object.hasOwn(value, "process_role") && Object.hasOwn(value, "temporality") && Object.hasOwn(value, "point_count") && Object.hasOwn(value, "point_start") && Object.hasOwn(value, "returned_points") && Object.hasOwn(value, "truncated") && Object.hasOwn(value, "points") && (typeof value["id"] === "string") && (typeof value["name"] === "string") && (typeof value["unit"] === "string") && (typeof value["scope"] === "string") && (typeof value["pid"] === "number" && Number.isInteger(value["pid"]) && value["pid"] >= 0) && (typeof value["tid"] === "string") && (typeof value["producer_id"] === "string") && ((typeof value["context_id"] === "string") || (value["context_id"] === null)) && (typeof value["thread_id_source"] === "string") && (typeof value["clock"] === "string") && (typeof value["clock_alignment"] === "string") && (typeof value["time_mapping"] === "string") && true && true && true && true && (typeof value["point_count"] === "number" && Number.isInteger(value["point_count"]) && value["point_count"] >= 0) && (typeof value["point_start"] === "number" && Number.isInteger(value["point_start"]) && value["point_start"] >= 0) && (typeof value["returned_points"] === "number" && Number.isInteger(value["returned_points"]) && value["returned_points"] >= 0) && (typeof value["truncated"] === "boolean") && (Array.isArray(value["points"]) && value["points"].every(item => isResourcePoint(item))));
}

export function isResources(value: unknown): value is Resources {
  return (isOutputObject(value) && Object.hasOwn(value, "series") && Object.hasOwn(value, "next_cursor") && Object.hasOwn(value, "returned_points") && Object.hasOwn(value, "total_points") && Object.hasOwn(value, "point_limit") && Object.hasOwn(value, "unaligned_point_count") && Object.hasOwn(value, "diagnostics") && (Array.isArray(value["series"]) && value["series"].every(item => isResourceSeries(item))) && ((typeof value["next_cursor"] === "string") || (value["next_cursor"] === null)) && (typeof value["returned_points"] === "number" && Number.isInteger(value["returned_points"]) && value["returned_points"] >= 0) && (typeof value["total_points"] === "number" && Number.isInteger(value["total_points"]) && value["total_points"] >= 0) && (typeof value["point_limit"] === "number" && Number.isInteger(value["point_limit"]) && value["point_limit"] >= 0) && (typeof value["unaligned_point_count"] === "number" && Number.isInteger(value["unaligned_point_count"]) && value["unaligned_point_count"] >= 0) && (Array.isArray(value["diagnostics"]) && value["diagnostics"].every(item => (typeof item === "string"))));
}

export function isContexts(value: unknown): value is Contexts {
  return (isOutputObject(value) && Object.hasOwn(value, "contexts") && Object.hasOwn(value, "relations") && Object.hasOwn(value, "next_cursor") && (Array.isArray(value["contexts"]) && value["contexts"].every(item => isContextObservation(item))) && (Array.isArray(value["relations"]) && value["relations"].every(item => isRelationObservation(item))) && ((typeof value["next_cursor"] === "string") || (value["next_cursor"] === null)));
}

export function isEvidence(value: unknown): value is Evidence {
  return (isOutputObject(value) && Object.hasOwn(value, "records") && (Array.isArray(value["records"]) && value["records"].every(item => isObservation(item))));
}

export function isComparisonMetric(value: unknown): value is ComparisonMetric {
  return (isOutputObject(value) && Object.hasOwn(value, "name") && Object.hasOwn(value, "unit") && Object.hasOwn(value, "scope") && Object.hasOwn(value, "basis") && Object.hasOwn(value, "before") && Object.hasOwn(value, "after") && Object.hasOwn(value, "delta") && Object.hasOwn(value, "percent_change") && (typeof value["name"] === "string") && (typeof value["unit"] === "string") && (typeof value["scope"] === "string") && (typeof value["basis"] === "string") && ((typeof value["before"] === "number" && Number.isFinite(value["before"])) || (value["before"] === null)) && ((typeof value["after"] === "number" && Number.isFinite(value["after"])) || (value["after"] === null)) && ((typeof value["delta"] === "number" && Number.isFinite(value["delta"])) || (value["delta"] === null)) && ((typeof value["percent_change"] === "number" && Number.isFinite(value["percent_change"])) || (value["percent_change"] === null)) && (!Object.hasOwn(value, "measurement_quality") || (typeof value["measurement_quality"] === "string")) && (!Object.hasOwn(value, "comparable") || (typeof value["comparable"] === "boolean")) && (!Object.hasOwn(value, "before_observations") || (typeof value["before_observations"] === "number" && Number.isInteger(value["before_observations"]) && value["before_observations"] >= 0)) && (!Object.hasOwn(value, "after_observations") || (typeof value["after_observations"] === "number" && Number.isInteger(value["after_observations"]) && value["after_observations"] >= 0)) && (!Object.hasOwn(value, "before_process_lifetimes") || (typeof value["before_process_lifetimes"] === "number" && Number.isInteger(value["before_process_lifetimes"]) && value["before_process_lifetimes"] >= 0)) && (!Object.hasOwn(value, "after_process_lifetimes") || (typeof value["after_process_lifetimes"] === "number" && Number.isInteger(value["after_process_lifetimes"]) && value["after_process_lifetimes"] >= 0)) && (!Object.hasOwn(value, "diagnostic") || (typeof value["diagnostic"] === "string")) && (!Object.hasOwn(value, "values_available") || (typeof value["values_available"] === "boolean")) && (!Object.hasOwn(value, "measurement_comparable") || (typeof value["measurement_comparable"] === "boolean")) && (!Object.hasOwn(value, "workload_comparable") || (typeof value["workload_comparable"] === "boolean")));
}

export function isComparisonHotspot(value: unknown): value is ComparisonHotspot {
  return (isOutputObject(value) && Object.hasOwn(value, "function") && Object.hasOwn(value, "file") && Object.hasOwn(value, "module") && Object.hasOwn(value, "before") && Object.hasOwn(value, "after") && Object.hasOwn(value, "delta") && Object.hasOwn(value, "unit") && Object.hasOwn(value, "basis") && Object.hasOwn(value, "before_available") && Object.hasOwn(value, "after_available") && (typeof value["function"] === "string") && ((typeof value["file"] === "string") || (value["file"] === null)) && ((typeof value["module"] === "string") || (value["module"] === null)) && ((typeof value["before"] === "number" && Number.isFinite(value["before"])) || (value["before"] === null)) && ((typeof value["after"] === "number" && Number.isFinite(value["after"])) || (value["after"] === null)) && ((typeof value["delta"] === "number" && Number.isFinite(value["delta"])) || (value["delta"] === null)) && (typeof value["unit"] === "string") && (typeof value["basis"] === "string") && (typeof value["before_available"] === "boolean") && (typeof value["after_available"] === "boolean") && (!Object.hasOwn(value, "presence") || ((value["presence"] === "both_observed" || value["presence"] === "before_only_observed" || value["presence"] === "after_only_observed" || value["presence"] === "unknown") && typeof value["presence"] === "string")));
}

export function isComparison(value: unknown): value is Comparison {
  return (isOutputObject(value) && Object.hasOwn(value, "before") && Object.hasOwn(value, "after") && Object.hasOwn(value, "comparable") && Object.hasOwn(value, "reasons") && Object.hasOwn(value, "metrics") && Object.hasOwn(value, "correctness") && Object.hasOwn(value, "hotspots") && Object.hasOwn(value, "sample_weight_units") && Object.hasOwn(value, "limitations") && isRun(value["before"]) && isRun(value["after"]) && (typeof value["comparable"] === "boolean") && (Array.isArray(value["reasons"]) && value["reasons"].every(item => (typeof item === "string"))) && (Array.isArray(value["metrics"]) && value["metrics"].every(item => isComparisonMetric(item))) && (isOutputObject(value["correctness"]) && Object.hasOwn(value["correctness"], "before") && Object.hasOwn(value["correctness"], "after") && true && true) && (Array.isArray(value["hotspots"]) && value["hotspots"].every(item => isComparisonHotspot(item))) && (isOutputObject(value["sample_weight_units"]) && Object.hasOwn(value["sample_weight_units"], "before") && Object.hasOwn(value["sample_weight_units"], "after") && (Array.isArray(value["sample_weight_units"]["before"]) && value["sample_weight_units"]["before"].every(item => (typeof item === "string"))) && (Array.isArray(value["sample_weight_units"]["after"]) && value["sample_weight_units"]["after"].every(item => (typeof item === "string")))) && (Array.isArray(value["limitations"]) && value["limitations"].every(item => (typeof item === "string"))) && (!Object.hasOwn(value, "hotspot_selection") || isComparisonHotspotSelection(value["hotspot_selection"])));
}

export function isOperationFailure(value: unknown): value is OperationFailure {
  return (isOutputObject(value) && Object.hasOwn(value, "code") && Object.hasOwn(value, "operation") && Object.hasOwn(value, "message") && ((value["code"] === "unknown_operation" || value["code"] === "invalid_parameters" || value["code"] === "conflict" || value["code"] === "operation_failed" || value["code"] === "capacity_exceeded" || value["code"] === "request_abandoned" || value["code"] === "request_deadline_exceeded" || value["code"] === "work_budget_exceeded") && typeof value["code"] === "string") && (typeof value["operation"] === "string") && (typeof value["message"] === "string") && (!Object.hasOwn(value, "capacity") || (isOutputObject(value["capacity"]) && Object.hasOwn(value["capacity"], "scope") && Object.hasOwn(value["capacity"], "maximum_in_flight") && (typeof value["capacity"]["scope"] === "string") && (typeof value["capacity"]["maximum_in_flight"] === "number" && Number.isInteger(value["capacity"]["maximum_in_flight"]) && value["capacity"]["maximum_in_flight"] >= 1) && Object.keys(value["capacity"]).every(key => ["scope","maximum_in_flight"].includes(key)))) && (!Object.hasOwn(value, "budget") || (isOutputObject(value["budget"]) && Object.hasOwn(value["budget"], "scope") && Object.hasOwn(value["budget"], "dimension") && Object.hasOwn(value["budget"], "limit") && Object.hasOwn(value["budget"], "observed") && (typeof value["budget"]["scope"] === "string") && (typeof value["budget"]["dimension"] === "string") && (typeof value["budget"]["limit"] === "number" && Number.isInteger(value["budget"]["limit"]) && value["budget"]["limit"] >= 0) && (typeof value["budget"]["observed"] === "number" && Number.isInteger(value["budget"]["observed"]) && value["budget"]["observed"] >= 0) && Object.keys(value["budget"]).every(key => ["scope","dimension","limit","observed"].includes(key)))) && Object.keys(value).every(key => ["code","operation","message","capacity","budget"].includes(key)));
}

export function isImportFormat(value: unknown): value is ImportFormat {
  return (isOutputObject(value) && Object.hasOwn(value, "format") && Object.hasOwn(value, "label") && Object.hasOwn(value, "description") && Object.hasOwn(value, "aliases") && Object.hasOwn(value, "requires") && Object.hasOwn(value, "requires_existing_recording") && (typeof value["format"] === "string" && [...value["format"]].length >= 1) && (typeof value["label"] === "string") && (typeof value["description"] === "string") && (Array.isArray(value["aliases"]) && value["aliases"].every(item => (typeof item === "string"))) && (Array.isArray(value["requires"]) && value["requires"].every(item => (typeof item === "string"))) && (!Object.hasOwn(value, "input_kind") || ((value["input_kind"] === "profile" || value["input_kind"] === "observations") && typeof value["input_kind"] === "string")) && (typeof value["requires_existing_recording"] === "boolean") && Object.keys(value).every(key => ["format","label","description","aliases","requires","input_kind","requires_existing_recording"].includes(key)));
}

export function isComparisonHotspotSelection(value: unknown): value is ComparisonHotspotSelection {
  return (isOutputObject(value) && Object.hasOwn(value, "weight_unit") && Object.hasOwn(value, "limit_per_recording") && Object.hasOwn(value, "before_available") && Object.hasOwn(value, "after_available") && Object.hasOwn(value, "before_total_functions") && Object.hasOwn(value, "after_total_functions") && Object.hasOwn(value, "before_truncated") && Object.hasOwn(value, "after_truncated") && (typeof value["weight_unit"] === "string") && (typeof value["limit_per_recording"] === "number" && Number.isInteger(value["limit_per_recording"]) && value["limit_per_recording"] >= 1) && (typeof value["before_available"] === "boolean") && (typeof value["after_available"] === "boolean") && (typeof value["before_total_functions"] === "number" && Number.isInteger(value["before_total_functions"]) && value["before_total_functions"] >= 0) && (typeof value["after_total_functions"] === "number" && Number.isInteger(value["after_total_functions"]) && value["after_total_functions"] >= 0) && (typeof value["before_truncated"] === "boolean") && (typeof value["after_truncated"] === "boolean") && Object.keys(value).every(key => ["weight_unit","limit_per_recording","before_available","after_available","before_total_functions","after_total_functions","before_truncated","after_truncated"].includes(key)));
}

export function isInspectionFinding(value: unknown): value is InspectionFinding {
  return (((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "missing_referenced_blob"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "sha256") && Object.hasOwn(value, "example_run_id") && Object.hasOwn(value, "example_artifact_id")))) : true)) && ((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "unreferenced_at_snapshot"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "sha256") && Object.hasOwn(value, "size_bytes")))) : true)) && ((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "pending_ownership_unknown"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "relative_path") && Object.hasOwn(value, "path_kind") && Object.hasOwn(value, "size_bytes")))) : true)) && ((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "workspace_ownership_unknown"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "relative_path") && Object.hasOwn(value, "path_kind") && Object.hasOwn(value, "metadata_reference")))) : true)) && ((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "unfollowed_path"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "relative_path") && Object.hasOwn(value, "path_kind")))) : true)) && ((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "path_unavailable"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "relative_path") && Object.hasOwn(value, "reason")))) : true)) && ((((!isOutputObject(value) || (isOutputObject(value) && (!Object.hasOwn(value, "kind") || (value["kind"] === "invalid_artifact_reference"))))) ? ((!isOutputObject(value) || (isOutputObject(value) && Object.hasOwn(value, "row_ordinal") && Object.hasOwn(value, "reason")))) : true)) && isOutputObject(value) && Object.hasOwn(value, "kind") && (!Object.hasOwn(value, "sha256") || (typeof value["sha256"] === "string")) && (!Object.hasOwn(value, "example_run_id") || (typeof value["example_run_id"] === "string")) && (!Object.hasOwn(value, "example_artifact_id") || (typeof value["example_artifact_id"] === "string")) && (!Object.hasOwn(value, "relative_path") || (typeof value["relative_path"] === "string")) && (!Object.hasOwn(value, "reason") || (typeof value["reason"] === "string")) && ((value["kind"] === "missing_referenced_blob" || value["kind"] === "unreferenced_at_snapshot" || value["kind"] === "pending_ownership_unknown" || value["kind"] === "workspace_ownership_unknown" || value["kind"] === "unfollowed_path" || value["kind"] === "path_unavailable" || value["kind"] === "invalid_artifact_reference") && typeof value["kind"] === "string") && (!Object.hasOwn(value, "path_kind") || ((value["path_kind"] === "file" || value["path_kind"] === "directory" || value["path_kind"] === "symlink" || value["path_kind"] === "other") && typeof value["path_kind"] === "string")) && (!Object.hasOwn(value, "metadata_reference") || ((value["metadata_reference"] === "present" || value["metadata_reference"] === "absent" || value["metadata_reference"] === "unknown") && typeof value["metadata_reference"] === "string")) && (!Object.hasOwn(value, "size_bytes") || ((typeof value["size_bytes"] === "string") || (value["size_bytes"] === null))) && (!Object.hasOwn(value, "row_ordinal") || (typeof value["row_ordinal"] === "number" && Number.isInteger(value["row_ordinal"]) && value["row_ordinal"] >= 1)) && Object.keys(value).every(key => ["sha256","example_run_id","example_artifact_id","relative_path","reason","kind","path_kind","metadata_reference","size_bytes","row_ordinal"].includes(key)));
}

export function isInspectionScan(value: unknown): value is InspectionScan {
  return (isOutputObject(value) && Object.hasOwn(value, "area") && Object.hasOwn(value, "examined") && Object.hasOwn(value, "status") && ((value["area"] === "artifact_references" || value["area"] === "artifact_files" || value["area"] === "captures" || value["area"] === "analyzer_executions" || value["area"] === "analyzer_packages") && typeof value["area"] === "string") && (typeof value["examined"] === "number" && Number.isInteger(value["examined"]) && value["examined"] >= 0) && ((value["status"] === "not_scanned" || value["status"] === "exhausted" || value["status"] === "limited" || value["status"] === "unavailable") && typeof value["status"] === "string") && Object.keys(value).every(key => ["area","examined","status"].includes(key)));
}

export function isStorageInspection(value: unknown): value is StorageInspection {
  return (isOutputObject(value) && Object.hasOwn(value, "findings") && Object.hasOwn(value, "scans") && Object.hasOwn(value, "limits_hit") && Object.hasOwn(value, "budgets") && Object.hasOwn(value, "elapsed_ms") && Object.hasOwn(value, "database_changed_during_scan") && Object.hasOwn(value, "content_hashes_checked") && Object.hasOwn(value, "deletion_authorized") && Object.hasOwn(value, "reference_view") && Object.hasOwn(value, "filesystem_view") && Object.hasOwn(value, "path_encoding") && Object.hasOwn(value, "limitations") && (Array.isArray(value["findings"]) && value["findings"].length <= 2000 && value["findings"].every(item => isInspectionFinding(item))) && (Array.isArray(value["scans"]) && value["scans"].length >= 5 && value["scans"].length <= 5 && value["scans"].every(item => isInspectionScan(item))) && (Array.isArray(value["limits_hit"]) && new Set(value["limits_hit"]).size === value["limits_hit"].length && value["limits_hit"].every(item => ((item === "artifact_rows" || item === "filesystem_entries" || item === "findings" || item === "time") && typeof item === "string"))) && (isOutputObject(value["budgets"]) && Object.hasOwn(value["budgets"], "artifact_row_limit") && Object.hasOwn(value["budgets"], "filesystem_entry_limit") && Object.hasOwn(value["budgets"], "finding_limit") && Object.hasOwn(value["budgets"], "budget_ms") && (typeof value["budgets"]["artifact_row_limit"] === "number" && Number.isInteger(value["budgets"]["artifact_row_limit"]) && value["budgets"]["artifact_row_limit"] >= 1 && value["budgets"]["artifact_row_limit"] <= 10000) && (typeof value["budgets"]["filesystem_entry_limit"] === "number" && Number.isInteger(value["budgets"]["filesystem_entry_limit"]) && value["budgets"]["filesystem_entry_limit"] >= 1 && value["budgets"]["filesystem_entry_limit"] <= 10000) && (typeof value["budgets"]["finding_limit"] === "number" && Number.isInteger(value["budgets"]["finding_limit"]) && value["budgets"]["finding_limit"] >= 1 && value["budgets"]["finding_limit"] <= 2000) && (typeof value["budgets"]["budget_ms"] === "number" && Number.isInteger(value["budgets"]["budget_ms"]) && value["budgets"]["budget_ms"] >= 1 && value["budgets"]["budget_ms"] <= 5000) && Object.keys(value["budgets"]).every(key => ["artifact_row_limit","filesystem_entry_limit","finding_limit","budget_ms"].includes(key))) && (typeof value["elapsed_ms"] === "number" && Number.isFinite(value["elapsed_ms"]) && value["elapsed_ms"] >= 0) && (typeof value["database_changed_during_scan"] === "boolean") && (value["content_hashes_checked"] === 0) && (value["deletion_authorized"] === false) && (value["reference_view"] === "sqlite_read_transaction") && (value["filesystem_view"] === "live_metadata") && (value["path_encoding"] === "lossy_display_only") && (Array.isArray(value["limitations"]) && value["limitations"].every(item => (typeof item === "string"))) && Object.keys(value).every(key => ["findings","scans","limits_hit","budgets","elapsed_ms","database_changed_during_scan","content_hashes_checked","deletion_authorized","reference_view","filesystem_view","path_encoding","limitations"].includes(key)));
}

export function isRunMetadata(value: unknown): value is RunMetadata {
  return (isOutputObject(value) && Object.hasOwn(value, "run") && isRun(value["run"]) && (!Object.hasOwn(value, "result_identity") || (typeof value["result_identity"] === "string")) && (!Object.hasOwn(value, "result_identity_diagnostic") || (typeof value["result_identity_diagnostic"] === "string")) && Object.keys(value).every(key => ["run","result_identity","result_identity_diagnostic"].includes(key)));
}

export function isRunRemoval(value: unknown): value is RunRemoval {
  return (isOutputObject(value) && Object.hasOwn(value, "receipt") && ((isOutputObject(value["receipt"])) || (value["receipt"] === null)) && Object.keys(value).every(key => ["receipt"].includes(key)));
}

export function isMutationSummary(value: unknown): value is MutationSummary {
  return ((isOutputObject(value) && Object.hasOwn(value, "sample_count") && Object.hasOwn(value, "total_weight") && Object.hasOwn(value, "weight_unit") && Object.hasOwn(value, "span_count") && Object.hasOwn(value, "metric_count") && Object.hasOwn(value, "relation_count") && Object.hasOwn(value, "diagnostics") && (typeof value["sample_count"] === "number" && Number.isInteger(value["sample_count"]) && value["sample_count"] >= 0) && ((typeof value["total_weight"] === "number" && Number.isFinite(value["total_weight"])) || (value["total_weight"] === null)) && (typeof value["weight_unit"] === "string") && (typeof value["span_count"] === "number" && Number.isInteger(value["span_count"]) && value["span_count"] >= 0) && (typeof value["metric_count"] === "number" && Number.isInteger(value["metric_count"]) && value["metric_count"] >= 0) && (typeof value["relation_count"] === "number" && Number.isInteger(value["relation_count"]) && value["relation_count"] >= 0) && (Array.isArray(value["diagnostics"]) && value["diagnostics"].every(item => (typeof item === "string")))) || (value === null));
}

export function isCountSelector(value: unknown): value is CountSelector {
  return (isOutputObject(value) && Object.hasOwn(value, "name") && (typeof value["name"] === "string" && [...value["name"]].length >= 1) && (!Object.hasOwn(value, "file") || (typeof value["file"] === "string" && [...value["file"]].length >= 1)) && (!Object.hasOwn(value, "line") || (typeof value["line"] === "number" && Number.isInteger(value["line"]) && value["line"] >= 1 && value["line"] <= 4294967295)) && Object.keys(value).every(key => ["name","file","line"].includes(key)));
}

export function isCountPlan(value: unknown): value is CountPlan {
  return (isOutputObject(value) && Object.hasOwn(value, "status") && Object.hasOwn(value, "method") && Object.hasOwn(value, "reason") && Object.hasOwn(value, "requested_scope") && Object.hasOwn(value, "collection_scope") && Object.hasOwn(value, "functions") && Object.hasOwn(value, "requires_rebuild") && Object.hasOwn(value, "limitations") && ((value["status"] === "ready" || value["status"] === "blocked")) && ((typeof value["method"] === "string") || (value["method"] === null)) && ((typeof value["reason"] === "string") || (value["reason"] === null)) && ((value["requested_scope"] === "all" || value["requested_scope"] === "selected")) && ((value["collection_scope"] === "all" || value["collection_scope"] === "selected" || value["collection_scope"] === "all_then_filter")) && (Array.isArray(value["functions"]) && value["functions"].every(item => isCountSelector(item))) && (typeof value["requires_rebuild"] === "boolean") && (Array.isArray(value["limitations"]) && value["limitations"].every(item => (typeof item === "string"))) && Object.keys(value).every(key => ["status","method","reason","requested_scope","collection_scope","functions","requires_rebuild","limitations"].includes(key)));
}

export function isCountPlanResult(value: unknown): value is CountPlanResult {
  return (isOutputObject(value) && Object.hasOwn(value, "plan") && isCountPlan(value["plan"]) && Object.keys(value).every(key => ["plan"].includes(key)));
}

export function isCountMeasurement(value: unknown): value is CountMeasurement {
  return (isOutputObject(value) && Object.hasOwn(value, "plan") && Object.hasOwn(value, "parent_run_id") && Object.hasOwn(value, "reused") && (!Object.hasOwn(value, "run") || isRun(value["run"])) && isCountPlan(value["plan"]) && (typeof value["parent_run_id"] === "string") && (typeof value["reused"] === "boolean") && Object.keys(value).every(key => ["run","plan","parent_run_id","reused"].includes(key)));
}

export function isCountFunction(value: unknown): value is CountFunction {
  return (isOutputObject(value) && Object.hasOwn(value, "name") && Object.hasOwn(value, "file") && Object.hasOwn(value, "line") && (!Object.hasOwn(value, "code_id") || (typeof value["code_id"] === "string")) && (typeof value["name"] === "string") && ((typeof value["file"] === "string") || (value["file"] === null)) && ((typeof value["line"] === "number" && Number.isInteger(value["line"]) && value["line"] >= 1) || (value["line"] === null)) && Object.keys(value).every(key => ["code_id","name","file","line"].includes(key)));
}

export function isFunctionCount(value: unknown): value is FunctionCount {
  return (isOutputObject(value) && Object.hasOwn(value, "function") && Object.hasOwn(value, "calls") && Object.hasOwn(value, "resumes") && isCountFunction(value["function"]) && (typeof value["calls"] === "number" && Number.isInteger(value["calls"]) && value["calls"] >= 0 && value["calls"] <= 9007199254740991) && ((typeof value["resumes"] === "number" && Number.isInteger(value["resumes"]) && value["resumes"] >= 0 && value["resumes"] <= 9007199254740991) || (value["resumes"] === null)) && Object.keys(value).every(key => ["function","calls","resumes"].includes(key)));
}

export function isCallCounts(value: unknown): value is CallCounts {
  return (isOutputObject(value) && Object.hasOwn(value, "status") && Object.hasOwn(value, "run_id") && Object.hasOwn(value, "parent_run_id") && Object.hasOwn(value, "method") && Object.hasOwn(value, "requested_scope") && Object.hasOwn(value, "collection_scope") && Object.hasOwn(value, "counts") && Object.hasOwn(value, "next_cursor") && Object.hasOwn(value, "total_count") && Object.hasOwn(value, "diagnostics") && Object.hasOwn(value, "limitations") && ((value["status"] === "not_collected" || value["status"] === "running" || value["status"] === "complete" || value["status"] === "incomplete" || value["status"] === "failed" || value["status"] === "canceled")) && (typeof value["run_id"] === "string") && ((typeof value["parent_run_id"] === "string") || (value["parent_run_id"] === null)) && ((typeof value["method"] === "string") || (value["method"] === null)) && ((value["requested_scope"] === "all" || value["requested_scope"] === "selected")) && ((value["collection_scope"] === "all" || value["collection_scope"] === "selected" || value["collection_scope"] === "all_then_filter")) && (Array.isArray(value["counts"]) && value["counts"].every(item => isFunctionCount(item))) && ((typeof value["next_cursor"] === "string") || (value["next_cursor"] === null)) && (typeof value["total_count"] === "number" && Number.isInteger(value["total_count"]) && value["total_count"] >= 0) && (Array.isArray(value["diagnostics"]) && value["diagnostics"].every(item => (typeof item === "string"))) && (Array.isArray(value["limitations"]) && value["limitations"].every(item => (typeof item === "string"))) && Object.keys(value).every(key => ["status","run_id","parent_run_id","method","requested_scope","collection_scope","counts","next_cursor","total_count","diagnostics","limitations"].includes(key)));
}

export function isProfileInspectionFunction(value: unknown): value is ProfileInspectionFunction {
  return (isOutputObject(value) && Object.hasOwn(value, "id") && Object.hasOwn(value, "function") && Object.hasOwn(value, "file") && Object.hasOwn(value, "line") && Object.hasOwn(value, "module") && Object.hasOwn(value, "code_id") && Object.hasOwn(value, "source_position_basis") && Object.hasOwn(value, "self_weight") && Object.hasOwn(value, "inclusive_weight") && Object.hasOwn(value, "weight_unit") && (typeof value["id"] === "string") && (typeof value["function"] === "string") && ((typeof value["file"] === "string") || (value["file"] === null)) && ((typeof value["line"] === "number" && Number.isInteger(value["line"])) || (value["line"] === null)) && ((typeof value["module"] === "string") || (value["module"] === null)) && ((typeof value["code_id"] === "string") || (value["code_id"] === null)) && (typeof value["source_position_basis"] === "string") && ((typeof value["self_weight"] === "number" && Number.isFinite(value["self_weight"])) || (value["self_weight"] === null)) && ((typeof value["inclusive_weight"] === "number" && Number.isFinite(value["inclusive_weight"])) || (value["inclusive_weight"] === null)) && (typeof value["weight_unit"] === "string"));
}

export function isProfileInspectionCaller(value: unknown): value is ProfileInspectionCaller {
  return (isOutputObject(value) && Object.hasOwn(value, "id") && Object.hasOwn(value, "function") && Object.hasOwn(value, "file") && Object.hasOwn(value, "line") && Object.hasOwn(value, "module") && Object.hasOwn(value, "code_id") && Object.hasOwn(value, "weight") && Object.hasOwn(value, "sample_count") && (typeof value["id"] === "string") && (typeof value["function"] === "string") && ((typeof value["file"] === "string") || (value["file"] === null)) && ((typeof value["line"] === "number" && Number.isInteger(value["line"])) || (value["line"] === null)) && ((typeof value["module"] === "string") || (value["module"] === null)) && ((typeof value["code_id"] === "string") || (value["code_id"] === null)) && ((typeof value["weight"] === "number" && Number.isFinite(value["weight"])) || (value["weight"] === null)) && (typeof value["sample_count"] === "number" && Number.isInteger(value["sample_count"]) && value["sample_count"] >= 0 && value["sample_count"] <= 9007199254740991));
}

export function isProfileInspectionNode(value: unknown): value is ProfileInspectionNode {
  return (isOutputObject(value) && Object.hasOwn(value, "id") && Object.hasOwn(value, "name") && Object.hasOwn(value, "file") && Object.hasOwn(value, "line") && Object.hasOwn(value, "module") && Object.hasOwn(value, "code_id") && Object.hasOwn(value, "source_position_basis") && Object.hasOwn(value, "self_weight") && Object.hasOwn(value, "total_weight") && Object.hasOwn(value, "child_count") && Object.hasOwn(value, "children_truncated") && Object.hasOwn(value, "depth_truncated") && Object.hasOwn(value, "children") && Object.hasOwn(value, "frame_id") && (typeof value["id"] === "string") && (typeof value["name"] === "string") && ((typeof value["file"] === "string") || (value["file"] === null)) && ((typeof value["line"] === "number" && Number.isInteger(value["line"])) || (value["line"] === null)) && ((typeof value["module"] === "string") || (value["module"] === null)) && ((typeof value["code_id"] === "string") || (value["code_id"] === null)) && (typeof value["source_position_basis"] === "string") && ((typeof value["self_weight"] === "number" && Number.isFinite(value["self_weight"])) || (value["self_weight"] === null)) && ((typeof value["total_weight"] === "number" && Number.isFinite(value["total_weight"])) || (value["total_weight"] === null)) && (typeof value["child_count"] === "number" && Number.isInteger(value["child_count"]) && value["child_count"] >= 0) && (typeof value["children_truncated"] === "boolean") && (typeof value["depth_truncated"] === "boolean") && (Array.isArray(value["children"]) && value["children"].every(item => isProfileInspectionNode(item))) && (typeof value["frame_id"] === "string"));
}

export function isProfileInspectionSelection(value: unknown): value is ProfileInspectionSelection {
  return ((isOutputObject(value) && Object.hasOwn(value, "id") && Object.hasOwn(value, "function") && Object.hasOwn(value, "file") && Object.hasOwn(value, "line") && Object.hasOwn(value, "module") && Object.hasOwn(value, "code_id") && Object.hasOwn(value, "source_position_basis") && Object.hasOwn(value, "self_weight") && Object.hasOwn(value, "inclusive_weight") && Object.hasOwn(value, "weight_unit") && Object.hasOwn(value, "sample_count") && Object.hasOwn(value, "observed_start_us") && Object.hasOwn(value, "observed_end_us") && Object.hasOwn(value, "callers") && Object.hasOwn(value, "evidence_ids") && Object.hasOwn(value, "evidence_count") && Object.hasOwn(value, "evidence_truncated") && Object.hasOwn(value, "callers_truncated") && (typeof value["id"] === "string") && (typeof value["function"] === "string") && ((typeof value["file"] === "string") || (value["file"] === null)) && ((typeof value["line"] === "number" && Number.isInteger(value["line"])) || (value["line"] === null)) && ((typeof value["module"] === "string") || (value["module"] === null)) && ((typeof value["code_id"] === "string") || (value["code_id"] === null)) && (typeof value["source_position_basis"] === "string") && ((typeof value["self_weight"] === "number" && Number.isFinite(value["self_weight"])) || (value["self_weight"] === null)) && ((typeof value["inclusive_weight"] === "number" && Number.isFinite(value["inclusive_weight"])) || (value["inclusive_weight"] === null)) && (typeof value["weight_unit"] === "string") && (typeof value["sample_count"] === "number" && Number.isInteger(value["sample_count"]) && value["sample_count"] >= 0 && value["sample_count"] <= 9007199254740991) && (typeof value["observed_start_us"] === "number" && Number.isInteger(value["observed_start_us"]) && value["observed_start_us"] >= 0 && value["observed_start_us"] <= 9007199254740991) && (typeof value["observed_end_us"] === "number" && Number.isInteger(value["observed_end_us"]) && value["observed_end_us"] >= 0 && value["observed_end_us"] <= 9007199254740991) && (Array.isArray(value["callers"]) && value["callers"].every(item => isProfileInspectionCaller(item))) && (Array.isArray(value["evidence_ids"]) && value["evidence_ids"].length <= 50 && value["evidence_ids"].every(item => (typeof item === "string"))) && (typeof value["evidence_count"] === "number" && Number.isInteger(value["evidence_count"]) && value["evidence_count"] >= 0 && value["evidence_count"] <= 9007199254740991) && (typeof value["evidence_truncated"] === "boolean") && (typeof value["callers_truncated"] === "boolean")) || (value === null));
}

export function isProfileInspectionBin(value: unknown): value is ProfileInspectionBin {
  return (isOutputObject(value) && Object.hasOwn(value, "start_us") && Object.hasOwn(value, "end_us") && Object.hasOwn(value, "weight") && Object.hasOwn(value, "selected_weight") && Object.hasOwn(value, "sample_count") && (typeof value["start_us"] === "number" && Number.isInteger(value["start_us"]) && value["start_us"] >= 0 && value["start_us"] <= 9007199254740991) && (typeof value["end_us"] === "number" && Number.isInteger(value["end_us"]) && value["end_us"] >= 0 && value["end_us"] <= 9007199254740991) && ((typeof value["weight"] === "number" && Number.isFinite(value["weight"])) || (value["weight"] === null)) && ((typeof value["selected_weight"] === "number" && Number.isFinite(value["selected_weight"])) || (value["selected_weight"] === null)) && (typeof value["sample_count"] === "number" && Number.isInteger(value["sample_count"]) && value["sample_count"] >= 0 && value["sample_count"] <= 9007199254740991));
}

export function isProfileInspection(value: unknown): value is ProfileInspection {
  return (isOutputObject(value) && Object.hasOwn(value, "result_identity") && Object.hasOwn(value, "weight_unit") && Object.hasOwn(value, "units") && Object.hasOwn(value, "bounds") && Object.hasOwn(value, "window") && Object.hasOwn(value, "sample_count") && Object.hasOwn(value, "total_sample_count") && Object.hasOwn(value, "total_weight") && Object.hasOwn(value, "capture_weight") && Object.hasOwn(value, "unaligned_count") && Object.hasOwn(value, "overview") && Object.hasOwn(value, "activity") && Object.hasOwn(value, "functions") && Object.hasOwn(value, "total_functions") && Object.hasOwn(value, "functions_truncated") && Object.hasOwn(value, "roots") && Object.hasOwn(value, "tree_truncated") && Object.hasOwn(value, "selected") && Object.hasOwn(value, "diagnostics") && (typeof value["result_identity"] === "string") && (typeof value["weight_unit"] === "string") && (Array.isArray(value["units"]) && value["units"].every(item => (typeof item === "string"))) && ((isOutputObject(value["bounds"]) && Object.hasOwn(value["bounds"], "start_us") && Object.hasOwn(value["bounds"], "end_us") && (typeof value["bounds"]["start_us"] === "number" && Number.isInteger(value["bounds"]["start_us"]) && value["bounds"]["start_us"] >= 0 && value["bounds"]["start_us"] <= 9007199254740991) && (typeof value["bounds"]["end_us"] === "number" && Number.isInteger(value["bounds"]["end_us"]) && value["bounds"]["end_us"] >= 0 && value["bounds"]["end_us"] <= 9007199254740991)) || (value["bounds"] === null)) && ((isOutputObject(value["window"]) && Object.hasOwn(value["window"], "start_us") && Object.hasOwn(value["window"], "end_us") && (typeof value["window"]["start_us"] === "number" && Number.isInteger(value["window"]["start_us"]) && value["window"]["start_us"] >= 0 && value["window"]["start_us"] <= 9007199254740991) && (typeof value["window"]["end_us"] === "number" && Number.isInteger(value["window"]["end_us"]) && value["window"]["end_us"] >= 0 && value["window"]["end_us"] <= 9007199254740991)) || (value["window"] === null)) && (typeof value["sample_count"] === "number" && Number.isInteger(value["sample_count"]) && value["sample_count"] >= 0 && value["sample_count"] <= 9007199254740991) && (typeof value["total_sample_count"] === "number" && Number.isInteger(value["total_sample_count"]) && value["total_sample_count"] >= 0 && value["total_sample_count"] <= 9007199254740991) && ((typeof value["total_weight"] === "number" && Number.isFinite(value["total_weight"])) || (value["total_weight"] === null)) && ((typeof value["capture_weight"] === "number" && Number.isFinite(value["capture_weight"])) || (value["capture_weight"] === null)) && (typeof value["unaligned_count"] === "number" && Number.isInteger(value["unaligned_count"]) && value["unaligned_count"] >= 0 && value["unaligned_count"] <= 9007199254740991) && (Array.isArray(value["overview"]) && value["overview"].every(item => isProfileInspectionBin(item))) && (Array.isArray(value["activity"]) && value["activity"].every(item => isProfileInspectionBin(item))) && (Array.isArray(value["functions"]) && value["functions"].every(item => isProfileInspectionFunction(item))) && (typeof value["total_functions"] === "number" && Number.isInteger(value["total_functions"]) && value["total_functions"] >= 0 && value["total_functions"] <= 9007199254740991) && (typeof value["functions_truncated"] === "boolean") && (Array.isArray(value["roots"]) && value["roots"].every(item => isProfileInspectionNode(item))) && (typeof value["tree_truncated"] === "boolean") && isProfileInspectionSelection(value["selected"]) && (Array.isArray(value["diagnostics"]) && value["diagnostics"].every(item => (typeof item === "string"))));
}

// Every operation result is checked against its declared wire contract.
export function validateOperationOutput(name: string, value: unknown): void {
  switch (name) {
    case "capabilities":
      if (!isCapabilities(value)) throw new Error("Hunin returned an invalid capabilities result.");
      break;
    case "storage.inspect":
      if (!isStorageInspection(value)) throw new Error("Hunin returned an invalid storage.inspect result.");
      break;
    case "runs.list":
      if (!isRunPage(value)) throw new Error("Hunin returned an invalid runs.list result.");
      break;
    case "runs.metadata":
      if (!isRunMetadata(value)) throw new Error("Hunin returned an invalid runs.metadata result.");
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
    case "runs.removal":
      if (!isRunRemoval(value)) throw new Error("Hunin returned an invalid runs.removal result.");
      break;
    case "profiles.hotspots":
      if (!isProfileHotspots(value)) throw new Error("Hunin returned an invalid profiles.hotspots result.");
      break;
    case "profiles.tree":
      if (!isProfileTree(value)) throw new Error("Hunin returned an invalid profiles.tree result.");
      break;
    case "profiles.inspect":
      if (!isProfileInspection(value)) throw new Error("Hunin returned an invalid profiles.inspect result.");
      break;
    case "timeline.query":
      if (!isTimeline(value)) throw new Error("Hunin returned an invalid timeline.query result.");
      break;
    case "resources.query":
      if (!isResources(value)) throw new Error("Hunin returned an invalid resources.query result.");
      break;
    case "contexts.query":
      if (!isContexts(value)) throw new Error("Hunin returned an invalid contexts.query result.");
      break;
    case "evidence.get":
      if (!isEvidence(value)) throw new Error("Hunin returned an invalid evidence.get result.");
      break;
    case "artifacts.list":
      if (!(isOutputObject(value) && Object.hasOwn(value, "artifacts") && Object.hasOwn(value, "next_cursor") && (Array.isArray(value["artifacts"]) && value["artifacts"].every(item => (isOutputObject(item) && Object.hasOwn(item, "id") && Object.hasOwn(item, "name") && Object.hasOwn(item, "size") && Object.hasOwn(item, "sha256") && Object.hasOwn(item, "format") && (typeof item["id"] === "string") && (typeof item["name"] === "string") && (typeof item["size"] === "number" && Number.isInteger(item["size"]) && item["size"] >= 0) && (typeof item["sha256"] === "string") && (typeof item["format"] === "string")))) && ((typeof value["next_cursor"] === "string") || (value["next_cursor"] === null)))) throw new Error("Hunin returned an invalid artifacts.list result.");
      break;
    case "artifacts.read":
      if (!(isOutputObject(value) && Object.hasOwn(value, "content") && Object.hasOwn(value, "encoding") && Object.hasOwn(value, "next_offset") && Object.hasOwn(value, "size") && (typeof value["content"] === "string") && (value["encoding"] === "utf8_lossy") && ((typeof value["next_offset"] === "number" && Number.isInteger(value["next_offset"]) && value["next_offset"] >= 0) || (value["next_offset"] === null)) && (typeof value["size"] === "number" && Number.isInteger(value["size"]) && value["size"] >= 0))) throw new Error("Hunin returned an invalid artifacts.read result.");
      break;
    case "source.read":
      if (!isSourceRead(value)) throw new Error("Hunin returned an invalid source.read result.");
      break;
    case "recordings.import":
      if (!isImportResult(value)) throw new Error("Hunin returned an invalid recordings.import result.");
      break;
    case "comparisons.create":
      if (!isComparison(value)) throw new Error("Hunin returned an invalid comparisons.create result.");
      break;
    case "analyses.list":
      if (!isAnalysisPage(value)) throw new Error("Hunin returned an invalid analyses.list result.");
      break;
    case "analyses.register":
      if (!isAnalysisRegistered(value)) throw new Error("Hunin returned an invalid analyses.register result.");
      break;
    case "analyses.run":
      if (!isAnalysisStarted(value)) throw new Error("Hunin returned an invalid analyses.run result.");
      break;
    case "analyses.results":
      if (!isAnalysisResultPage(value)) throw new Error("Hunin returned an invalid analyses.results result.");
      break;
    case "scenarios.list":
      if (!isScenarioPage(value)) throw new Error("Hunin returned an invalid scenarios.list result.");
      break;
    case "scenarios.save":
      if (!isScenarioSaveResult(value)) throw new Error("Hunin returned an invalid scenarios.save result.");
      break;
    case "scenarios.run":
      if (!isRunResult(value)) throw new Error("Hunin returned an invalid scenarios.run result.");
      break;
    case "analyses.cancel":
      if (!isAnalysisCanceled(value)) throw new Error("Hunin returned an invalid analyses.cancel result.");
      break;
    case "recordings.reanalyze":
      if (!isReanalysisResult(value)) throw new Error("Hunin returned an invalid recordings.reanalyze result.");
      break;
    case "calls.plan":
      if (!isCountPlanResult(value)) throw new Error("Hunin returned an invalid calls.plan result.");
      break;
    case "calls.measure":
      if (!isCountMeasurement(value)) throw new Error("Hunin returned an invalid calls.measure result.");
      break;
    case "calls.query":
      if (!isCallCounts(value)) throw new Error("Hunin returned an invalid calls.query result.");
      break;
    default: throw new Error(`No output contract for operation: ${name}`);
  }
}

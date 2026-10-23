// Generated from protocol/record.schema.json. Run npm run generate; do not edit.
type RecordBase = {
  "schema_version": 1;
  "kind": "sample" | "span" | "event" | "metric" | "relation" | "context";
  "id": string;
  "run_id": string;
  "producer_id": string;
  "pid": number;
  "tid": string;
  "timestamp_us": number;
  "name"?: string;
  "duration_us"?: number;
  "parent_id"?: string;
  "context_id"?: string;
  "stack"?: Array<{
  "function": string;
  "file"?: string;
  "line"?: number;
  "column"?: number;
  "module"?: string;
  "address"?: string;
  "code_id"?: string;
}>;
  "weight"?: number;
  "weight_unit"?: "samples" | "microseconds" | "bytes" | "count";
  "interval_us"?: number;
  "value"?: number;
  "unit"?: string;
  "scope"?: "process" | "thread" | "context" | "run";
  "from"?: string;
  "to"?: string;
  "relation"?: string;
  "evidence"?: Array<string>;
  "basis": "observed" | "sampled" | "explicit" | "derived";
  "attributes"?: Record<string, unknown>;
};

export type Observation = RecordBase & (
  { kind: "sample" } & Required<Pick<RecordBase, "stack" | "weight" | "weight_unit">>
  | { kind: "span" } & Required<Pick<RecordBase, "name" | "duration_us">>
  | { kind: "event" } & Required<Pick<RecordBase, "name">>
  | { kind: "metric" } & Required<Pick<RecordBase, "name" | "value" | "unit" | "scope">>
  | { kind: "relation" } & Required<Pick<RecordBase, "from" | "to" | "relation" | "evidence">>
  | { kind: "context" } & Required<Pick<RecordBase, "name">>
);

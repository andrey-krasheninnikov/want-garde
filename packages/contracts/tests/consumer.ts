import type {
  Action,
  OperationRequest,
  Job,
  HostSnapshot,
  OperationResult,
} from "../generated/index";
import { CONTRACT_VERSION } from "../generated/index";

const request: OperationRequest = {
  version: CONTRACT_VERSION,
  request_id: "request-001",
  server_id: "server-demo",
  action: { type: "server.status" },
};
const job: Job = {
  version: CONTRACT_VERSION,
  id: "job-001",
  request_id: request.request_id,
  server_id: request.server_id,
  state: { type: "needs_reconciliation", reason_key: "job.outcome_unknown" },
};

// @ts-expect-error Unknown operations need an explicit contract change.
const unknown: Action = { type: "root.shell" };
// @ts-expect-error Client JSON cannot grant authority.
const forged: OperationRequest = { ...request, approved: true };
void [job, unknown, forged];

const snapshot: HostSnapshot = {
  observed_at: "2026-01-01T00:00:00Z",
  hostname: { type: "available", value: "test-host" },
  os: { type: "unavailable", reason_key: "source.unavailable" },
  kernel: { type: "unavailable", reason_key: "source.unavailable" },
  uptime_seconds: { type: "available", value: 0 },
  cpu_usage_percent: { type: "available", value: 0 },
  load_average: { type: "available", value: [0, 0, 0] },
  memory: { type: "available", value: { total_bytes: 1048576, available_bytes: 524288 } },
  root_disk: { type: "unavailable", reason_key: "source.unavailable" },
};
const result: OperationResult = {
  type: "server_status",
  server_id: "test-host",
  product_version: "0.1.0-dev.0",
  snapshot,
};
// @ts-expect-error Unavailable measurements cannot contain a guessed zero.
const invalid: HostSnapshot["uptime_seconds"] = { type: "unavailable", value: 0 };
void [result, invalid];

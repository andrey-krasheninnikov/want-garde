import type { Action, OperationRequest, Job } from "../generated/index";
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

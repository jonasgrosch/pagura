/** worker_threads entry for host_test.ts: the browser worker's runHost over a MessagePort. */
import { parentPort, workerData } from 'node:worker_threads';
import { failureReason } from '../web/failure.mjs';
import { runHost } from '../web/worker.mjs';

// A worker always has a parent port.
const port = parentPort!;
try {
  await runHost(workerData.module, workerData.shared, (message, transfer = []) => port.postMessage(message, transfer));
} catch (error) {
  port.postMessage({ type: 'fatal', reason: failureReason(error), message: String(error) });
}

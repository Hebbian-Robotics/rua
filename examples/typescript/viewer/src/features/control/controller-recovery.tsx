import { apiSchemas } from "@hebbian/robot-use-client";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import { Alert } from "@/components/ui/alert.js";
import { Button } from "@/components/ui/button.js";
import { stationJson } from "../../api.js";
import { errorMessage } from "../../errors.js";

const controllerStatusSchema = apiSchemas.shape.ControllerStatus;

function lastReport(reports: string[]): string | null {
  const report = reports.at(-1);
  if (!report) return null;
  try {
    const guard = z.object({ guard: z.string() }).safeParse(JSON.parse(report));
    return guard.success ? guard.data.guard : report;
  } catch {
    return report;
  }
}

// SAFETY: Recovery controls must remain available when the controller no longer supplies state.
export function ControllerRecovery() {
  const queryClient = useQueryClient();
  const controllerQuery = useQuery({
    queryKey: ["controller-status"],
    queryFn: async () =>
      controllerStatusSchema.parse(await stationJson("api/controller")),
    refetchInterval: 1000,
    refetchIntervalInBackground: true,
    retry: false,
  });
  const status = controllerQuery.isError
    ? null
    : (controllerQuery.data ?? null);
  const [supported, setSupported] = useState(false);
  const [recovering, setRecovering] = useState(false);

  const reconnect = useMutation({
    mutationFn: async () => {
      await stationJson("api/controller/reconnect", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ supported: true }),
      });
    },
    onSuccess: () => {
      setRecovering(true);
      if (status?.phase === "unavailable")
        queryClient.setQueryData(["controller-status"], {
          phase: "waiting_for_guard",
          reason: status.reason,
          reports: status.reports,
        });
      setSupported(false);
    },
  });
  const error = reconnect.error
    ? `Could not reconnect: ${errorMessage(reconnect.error)}`
    : "";

  if (!status || status.phase === "unmanaged") return null;
  if (status.phase === "online") {
    return recovering ? (
      <Alert className="mx-6 mb-3 w-auto" role="status">
        <p className="m-0">
          Arms reconnected and disabled. Enable each separately when ready.
        </p>
        <Button
          variant="outline"
          className="w-fit"
          onClick={() => setRecovering(false)}
        >
          Dismiss
        </Button>
      </Alert>
    ) : null;
  }
  if (status.phase === "starting") {
    return (
      <Alert className="mx-6 mb-3 w-auto" role="status">
        Checking the arm’s feedback and faults…
      </Alert>
    );
  }
  const report = lastReport(status.reports);
  return (
    <Alert
      variant="destructive"
      role="region"
      className="mx-6 mb-3 w-auto gap-2"
      aria-labelledby="controller-recovery-title"
    >
      <h2 id="controller-recovery-title">Arm controller unavailable</h2>
      <p className="m-0">{status.reason}</p>
      <p className="m-0">
        {status.phase === "waiting_for_guard"
          ? "Waiting for the safety guard to release the connection."
          : status.guard_released
            ? "The previous controller and any guard have exited. Motor torque cannot be confirmed until the arm is checked."
            : "The safety guard may be supervising the arm. Motor torque cannot be confirmed."}
      </p>
      {report && (
        <p className="m-0">Latest controller or guard report: {report}</p>
      )}
      {status.phase === "unavailable" && (
        <>
          <label className="my-2">
            <input
              type="checkbox"
              checked={supported}
              onChange={(event) => setSupported(event.currentTarget.checked)}
            />
            I have supported all affected arms.
          </label>
          <Button
            variant="outline"
            className="w-fit"
            disabled={!supported || reconnect.isPending}
            onClick={() => reconnect.mutate()}
          >
            {reconnect.isPending ? "Requesting…" : "Reconnect arm"}
          </Button>
          <p className="m-0">
            If the arm still cannot reconnect, check its power and USB
            connection. After supporting it, power cycle it if needed.
          </p>
        </>
      )}
      {error && <p role="alert">{error}</p>}
    </Alert>
  );
}

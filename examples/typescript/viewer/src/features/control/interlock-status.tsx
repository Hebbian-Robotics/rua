import type { ArmState } from "@hebbian/robot-use-client";
import { Alert } from "@/components/ui/alert.js";
import { Button } from "@/components/ui/button.js";
import { describeInterlock } from "../../control-logic.js";
import { errorMessage } from "../../errors.js";
import { armControl } from "../../manual-lease.js";

export function InterlockStatus({
  arm,
  onMessage,
}: {
  arm: ArmState;
  onMessage: (message: string) => void;
}) {
  if (arm.interlocks.length === 0) return null;
  const needsApproval = arm.interlocks.some(
    (interlock) => interlock.kind === "recovery_approval_required",
  );
  return (
    <Alert role="status" variant="destructive" className="gap-2">
      Motion inhibited: {arm.interlocks.map(describeInterlock).join(", ")}
      {arm.recovery_issue?.detail && `: ${arm.recovery_issue.detail}`}
      {needsApproval && (
        <Button
          variant="outline"
          className="w-fit whitespace-normal"
          onClick={() => {
            void armControl(arm.arm)
              .approveRecovery()
              .then((answer) => {
                if (answer.status === "refused") onMessage(answer.detail);
              })
              .catch((error: unknown) => onMessage(errorMessage(error)));
          }}
        >
          Arm is supported — approve recovery
        </Button>
      )}
    </Alert>
  );
}

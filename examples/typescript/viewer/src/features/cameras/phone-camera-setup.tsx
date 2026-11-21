import { useMutation } from "@tanstack/react-query";
import { apiSchemas } from "hebbian-rua";
import { useId, useState } from "react";
import { Alert } from "@/components/ui/alert.js";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import { stationJson } from "../../api.js";
import { errorMessage } from "../../errors.js";

function InviteValue({ label, value }: { label: string; value: string }) {
  const id = useId();
  return (
    <label htmlFor={id}>
      {label}
      <Input
        id={id}
        className="font-mono text-xs"
        value={value}
        readOnly
        autoComplete="off"
        spellCheck={false}
        onFocus={(event) => event.currentTarget.select()}
      />
    </label>
  );
}

export function PhoneCameraSetup() {
  const [label, setLabel] = useState("");
  const [copyStatus, setCopyStatus] = useState<"copied" | "select-link" | null>(
    null,
  );
  const inviteMutation = useMutation({
    mutationFn: async (cameraLabel: string) => {
      const invite = apiSchemas.shape.PhoneInvite.parse(
        await stationJson("api/phone-invites", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            label: cameraLabel
              .trim()
              .toLowerCase()
              .replaceAll(/[\s-]+/g, "_"),
          }),
        }),
      );
      const cameraPage = new URL(invite.url);
      if (cameraPage.protocol !== "https:" && cameraPage.protocol !== "http:") {
        throw new Error("The station returned an invalid camera page URL.");
      }
      const connection = new URLSearchParams(cameraPage.hash.slice(1));
      cameraPage.hash = "";
      return {
        ...invite,
        cameraPageUrl: cameraPage.href,
        serverUrl: connection.get("url"),
        publishingToken: connection.get("token"),
      };
    },
  });
  const invite = inviteMutation.data;
  const message = inviteMutation.error
    ? `Could not create the invite: ${errorMessage(inviteMutation.error)}`
    : "";
  async function copyInviteLink(url: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(url);
      setCopyStatus("copied");
    } catch {
      setCopyStatus("select-link");
    }
  }

  return (
    <div className="grid min-w-0 gap-3">
      <form
        aria-label="Create a phone camera invite"
        className="grid gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (inviteMutation.isPending) return;
          setCopyStatus(null);
          inviteMutation.mutate(label);
        }}
      >
        <label htmlFor="phone-camera-label">
          Camera location
          <Input
            id="phone-camera-label"
            value={label}
            onChange={(event) => setLabel(event.currentTarget.value)}
            placeholder="e.g. table_left"
            autoCapitalize="none"
            autoComplete="off"
            required
          />
        </label>
        <Button
          type="submit"
          className="w-fit"
          disabled={inviteMutation.isPending}
        >
          {inviteMutation.isPending ? "Creating…" : "Create invite"}
        </Button>
      </form>
      {message && (
        <Alert role="alert" variant="destructive">
          {message}
        </Alert>
      )}
      {invite && (
        <div className="grid gap-2">
          <img
            className="mx-auto max-w-full bg-white"
            width={230}
            src={invite.qr_code_image}
            alt={`QR code for camera ${invite.camera_name}`}
          />
          <p role="status" className="m-0 text-muted-foreground">
            Open the invite link or scan the QR code on your phone, then start
            streaming as {invite.camera_name}.
          </p>
          <div className="flex flex-wrap items-center gap-3">
            <a href={invite.url} target="_blank" rel="noreferrer">
              Open invite link
            </a>
            <Button
              type="button"
              variant="outline"
              onClick={() => void copyInviteLink(invite.url)}
            >
              Copy invite link
            </Button>
          </div>
          <p role="status" className="m-0 text-sm text-muted-foreground">
            {copyStatus === "copied" && "Invite link copied."}
            {copyStatus === "select-link" &&
              "Select and copy the invite link below."}
          </p>
          {copyStatus === "select-link" && (
            <InviteValue label="Invite link" value={invite.url} />
          )}
          {invite.serverUrl && invite.publishingToken && (
            <details className="min-w-0">
              <summary>Manual LiveKit connection</summary>
              <div className="mt-2 grid gap-2">
                <p className="m-0 text-sm text-muted-foreground">
                  Paste these values into the camera page and select Use
                  connection. The publishing token is private and expires after
                  one hour.
                </p>
                <a href={invite.cameraPageUrl} target="_blank" rel="noreferrer">
                  Open camera page
                </a>
                <InviteValue
                  label="LiveKit server URL"
                  value={invite.serverUrl}
                />
                <InviteValue
                  label="Publishing token"
                  value={invite.publishingToken}
                />
              </div>
            </details>
          )}
        </div>
      )}
    </div>
  );
}

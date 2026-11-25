import {
  QueryClient,
  QueryClientProvider,
  useQuery,
} from "@tanstack/react-query";
import { createRoot } from "react-dom/client";
import { agentApi } from "../agent-api.js";
import { useCameraRoom } from "../features/cameras/live-cameras.js";
import {
  RecordingControls,
  RecordingStatus,
  useControlRecording,
} from "../features/recordings/recording-controls.js";
import { recordingNavigationLinks } from "../features/recordings/recording-navigation.js";
import { useViewerPreferences } from "../features/settings/use-viewer-preferences.js";
import { WorkspaceApp } from "./workspace.js";

function Viewer() {
  const backendUrl = globalThis.location.origin;
  const health = useQuery({
    queryKey: ["viewer-health", backendUrl],
    queryFn: () => agentApi.health(backendUrl),
  });
  if (!health.data) {
    return health.isError ? (
      <p role="alert">
        Could not connect to the viewer backend. Check that it is running, then
        reload.
      </p>
    ) : (
      <p role="status">Connecting to the viewer backend…</p>
    );
  }
  return <FullViewer />;
}

function FullViewer() {
  const viewerPreferences = useViewerPreferences();
  const { preferences, updatePreferences } = viewerPreferences;
  const cameras = useCameraRoom();
  const recording = useControlRecording(preferences.recordWithControl);

  return (
    <WorkspaceApp
      viewerPreferences={viewerPreferences}
      cameras={cameras}
      navigationLinks={recordingNavigationLinks}
      statusIndicator={
        <RecordingStatus
          recording={recording}
          roomMetadata={cameras.roomMetadata}
        />
      }
      beforeAgentStart={recording.startWithControl}
      onManualControlStarted={recording.startWithControl}
      renderAdditionalControls={(run) => (
        <div className="border-t border-border p-4">
          <RecordingControls
            recording={recording}
            run={run}
            autoRecord={preferences.recordWithControl}
            onAutoRecordChange={(recordWithControl) =>
              updatePreferences({ ...preferences, recordWithControl })
            }
          />
        </div>
      )}
    />
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("Viewer root is missing");
const queryClient = new QueryClient();
createRoot(root).render(
  <QueryClientProvider client={queryClient}>
    <Viewer />
  </QueryClientProvider>,
);

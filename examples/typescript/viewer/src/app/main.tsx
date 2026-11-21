import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRoot } from "react-dom/client";
import { WorkspaceApp } from "./workspace.js";

const root = document.getElementById("root");
if (!root) throw new Error("Viewer root is missing");
const queryClient = new QueryClient();
createRoot(root).render(
  <QueryClientProvider client={queryClient}>
    <WorkspaceApp />
  </QueryClientProvider>,
);

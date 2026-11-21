import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRoot } from "react-dom/client";
import { RecordingsApp } from "../features/recordings/recordings-app.js";

const root = document.getElementById("root");
if (!root) throw new Error("Recordings root is missing");
const queryClient = new QueryClient();
createRoot(root).render(
  <QueryClientProvider client={queryClient}>
    <RecordingsApp />
  </QueryClientProvider>,
);

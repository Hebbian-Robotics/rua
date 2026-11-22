import { FilmIcon } from "lucide-react";
import {
  type ViewerNavigationLink,
  workspaceNavigationLinks,
} from "../../shared/navigation.js";

const [workspaceLink, settingsLink] = workspaceNavigationLinks;

export const recordingNavigationLinks = [
  workspaceLink,
  {
    page: "recordings",
    label: "Recordings",
    href: "recording.html",
    Icon: FilmIcon,
  },
  settingsLink,
] satisfies readonly ViewerNavigationLink[];

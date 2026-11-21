import {
  CircleIcon,
  FilmIcon,
  GitForkIcon,
  LayoutGridIcon,
  SettingsIcon,
  SquareIcon,
} from "lucide-react";
import { type ReactNode, useState } from "react";
import { Alert } from "@/components/ui/alert.js";
import { Badge } from "@/components/ui/badge.js";
import { Button } from "@/components/ui/button.js";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarSeparator,
  SidebarTrigger,
  useSidebar,
} from "@/components/ui/sidebar.js";
import { TooltipProvider } from "@/components/ui/tooltip.js";
import { cn } from "@/lib/utils.js";
import { describeInterlock, describeLease } from "../control-logic.js";
import { errorMessage } from "../errors.js";
import { armControl, manualLease } from "../manual-lease.js";
import { useStationState } from "../use-station-state.js";

const navigationLinks = [
  { page: "workspace", label: "Workspace", href: "./", Icon: LayoutGridIcon },
  {
    page: "recordings",
    label: "Recordings",
    href: "recording.html",
    Icon: FilmIcon,
  },
  {
    page: "settings",
    label: "Settings",
    href: "./#settings",
    Icon: SettingsIcon,
  },
  {
    page: null,
    label: "GitHub",
    href: "https://github.com/Hebbian-Robotics/rua",
    Icon: GitForkIcon,
  },
] as const;

type ViewerShellProps = {
  current: "workspace" | "recordings" | "settings";
  onStop?: () => void;
  children: ReactNode;
};

export function ViewerShell({ current, onStop, children }: ViewerShellProps) {
  return (
    <TooltipProvider>
      <SidebarProvider className="flex-col md:flex-row">
        <ViewerNavigation current={current} onStop={onStop} />
        <div className="min-w-0 flex-1">{children}</div>
      </SidebarProvider>
    </TooltipProvider>
  );
}

function ViewerNavigation({
  current,
  onStop,
}: Pick<ViewerShellProps, "current" | "onStop">) {
  const { isMobile, open, openMobile, setOpenMobile } = useSidebar();
  const { stationState, connected } = useStationState();
  const [stopMessage, setStopMessage] = useState("");
  async function stopArms(): Promise<void> {
    if (!stationState) return;
    onStop?.();
    setStopMessage("Stopping arms…");
    const results = await Promise.allSettled(
      stationState.arms.map((arm) => armControl(arm.arm).stop()),
    );
    const failures = results.flatMap((result, index) =>
      result.status === "rejected"
        ? [
            `${stationState.arms[index]?.arm ?? "Arm"}: ${errorMessage(result.reason)}`,
          ]
        : result.value.status === "refused"
          ? [
              `${stationState.arms[index]?.arm ?? "Arm"}: ${result.value.detail}`,
            ]
          : [],
    );
    setStopMessage(
      failures.length
        ? failures.join(" · ")
        : "Stop requested. Check arm status for the outcome.",
    );
  }
  const stopDisabled = !connected || !stationState?.arms.length;
  const showInlineFeedback = isMobile ? openMobile : open;
  const connectionLabel = connected ? "Station connected" : "Station offline";

  return (
    <>
      {isMobile && (
        <header className="sticky top-0 z-20 flex w-full items-center gap-2 border-b border-sidebar-border bg-sidebar px-3 py-2">
          <SidebarTrigger
            aria-label="Open navigation"
            aria-expanded={openMobile}
          />
          <StationConnection connected={connected} label={connectionLabel} />
          <StopArmsButton
            className="ml-auto"
            disabled={stopDisabled}
            onStop={stopArms}
          />
        </header>
      )}
      <Sidebar collapsible="icon">
        <SidebarHeader className="flex-row items-center group-data-[collapsible=icon]:flex-col max-md:pr-12">
          <SidebarMenu className="min-w-0 flex-1">
            <SidebarMenuItem>
              <SidebarMenuButton
                aria-label="Hebbian Robot Use workspace"
                render={<a href="./" />}
                onClick={() => setOpenMobile(false)}
              >
                <img
                  className="size-4 shrink-0 rounded-sm"
                  src="./hebbian-logo.svg"
                  alt=""
                />
                <span>
                  Hebbian <strong>Robot Use</strong>
                </span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
          {!isMobile && (
            <SidebarTrigger
              aria-expanded={open}
              aria-label={open ? "Collapse sidebar" : "Expand sidebar"}
            />
          )}
        </SidebarHeader>
        <SidebarContent>
          <nav aria-label="Main navigation" className="p-2">
            <SidebarMenu>
              {navigationLinks.map(({ page, label, href, Icon }) => (
                <SidebarMenuItem key={href}>
                  <SidebarMenuButton
                    isActive={current === page}
                    tooltip={label}
                    render={
                      <a
                        href={href}
                        aria-label={label}
                        aria-current={current === page ? "page" : undefined}
                      />
                    }
                    onClick={() => setOpenMobile(false)}
                  >
                    <Icon aria-hidden="true" />
                    <span>{label}</span>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              ))}
            </SidebarMenu>
          </nav>
        </SidebarContent>
        <SidebarSeparator className="mx-0" />
        <SidebarFooter
          aria-label="Station status"
          role="region"
          className="min-h-0 max-h-[65%]"
        >
          <StationConnection connected={connected} label={connectionLabel} />
          <div className="min-h-0 overflow-y-auto grid gap-1 text-xs wrap-anywhere text-muted-foreground group-data-[collapsible=icon]:hidden">
            <strong className="text-sidebar-foreground">
              {stationState?.robot ?? "Robot station"}
            </strong>
            <span>
              {connected
                ? describeLease(stationState?.lease ?? null, manualLease.label)
                : "Lease status unavailable"}
            </span>
            {connected &&
              stationState?.arms.map((arm) => (
                <span key={arm.arm}>
                  {arm.arm}: {arm.mode}
                  {arm.interlocks.length > 0 &&
                    ` (${arm.interlocks.map(describeInterlock).join(", ")})`}
                </span>
              ))}
          </div>
          <StopArmsButton
            className="group-data-[collapsible=icon]:size-8 group-data-[collapsible=icon]:p-0"
            disabled={stopDisabled}
            onStop={stopArms}
          />
          {showInlineFeedback && <StopFeedback message={stopMessage} />}
        </SidebarFooter>
      </Sidebar>
      {!showInlineFeedback && (
        <StopFeedback
          className="fixed right-3 bottom-3 z-30 max-w-[min(22rem,calc(100dvw-1.5rem))] shadow-md"
          message={stopMessage}
        />
      )}
    </>
  );
}

function StationConnection({
  connected,
  label,
}: {
  connected: boolean;
  label: string;
}) {
  return (
    <div role="status" title={label}>
      <Badge
        variant={connected ? "outline" : "destructive"}
        className="group-data-[collapsible=icon]:w-8 group-data-[collapsible=icon]:px-0"
      >
        <CircleIcon
          aria-hidden="true"
          className={connected ? "fill-positive text-positive" : "fill-current"}
        />
        <span className="group-data-[collapsible=icon]:sr-only">{label}</span>
      </Badge>
    </div>
  );
}

function StopArmsButton({
  className,
  disabled,
  onStop,
}: {
  className?: string;
  disabled: boolean;
  onStop: () => Promise<void>;
}) {
  return (
    <Button
      variant="destructive"
      aria-label="Stop arms"
      title="Stop arms"
      className={className}
      disabled={disabled}
      onClick={() => void onStop()}
    >
      <SquareIcon aria-hidden="true" className="fill-current" />
      <span className="group-data-[collapsible=icon]:sr-only">Stop arms</span>
    </Button>
  );
}

function StopFeedback({
  className,
  message,
}: {
  className?: string;
  message: string;
}) {
  return (
    <div role="status">
      {message && (
        <Alert
          role="presentation"
          className={cn("max-h-32 overflow-y-auto wrap-anywhere", className)}
        >
          {message}
        </Alert>
      )}
    </div>
  );
}

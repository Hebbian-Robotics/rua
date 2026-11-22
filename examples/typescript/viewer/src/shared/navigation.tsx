import {
  CircleIcon,
  LayoutGridIcon,
  type LucideIcon,
  SettingsIcon,
  SquareIcon,
} from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
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
import { errorMessage } from "../errors.js";
import { armControl } from "../manual-lease.js";
import { useStationState } from "../use-station-state.js";

export type ViewerNavigationLink = {
  page: string;
  label: string;
  href: string;
  Icon: LucideIcon;
};

export const workspaceNavigationLinks = [
  {
    page: "workspace",
    label: "Workspace",
    href: "./#workspace",
    Icon: LayoutGridIcon,
  },
  {
    page: "settings",
    label: "Settings",
    href: "./#settings",
    Icon: SettingsIcon,
  },
] as const satisfies readonly ViewerNavigationLink[];

function ViewerFooter() {
  return (
    <footer className="viewer-footer">
      <a
        href="https://github.com/Hebbian-Robotics/rua"
        className="viewer-github-link"
      >
        <svg
          aria-hidden="true"
          viewBox="0 0 32 32"
          width="20"
          height="20"
          fill="currentColor"
        >
          <path
            fillRule="evenodd"
            clipRule="evenodd"
            d="M16 0C7.16 0 0 7.16 0 16C0 23.08 4.58 29.06 10.94 31.18C11.74 31.32 12.04 30.84 12.04 30.42C12.04 30.04 12.02 28.78 12.02 27.44C8 28.18 6.96 26.46 6.64 25.56C6.46 25.1 5.68 23.68 5 23.3C4.44 23 3.64 22.26 4.98 22.24C6.24 22.22 7.14 23.4 7.44 23.88C8.88 26.3 11.18 25.62 12.1 25.2C12.24 24.16 12.66 23.46 13.12 23.06C9.56 22.66 5.84 21.28 5.84 15.16C5.84 13.42 6.46 11.98 7.48 10.86C7.32 10.46 6.76 8.82 7.64 6.62C7.64 6.62 8.98 6.2 12.04 8.26C13.32 7.9 14.68 7.72 16.04 7.72C17.4 7.72 18.76 7.9 20.04 8.26C23.1 6.18 24.44 6.62 24.44 6.62C25.32 8.82 24.76 10.46 24.6 10.86C25.62 11.98 26.24 13.4 26.24 15.16C26.24 21.3 22.5 22.66 18.94 23.06C19.52 23.56 20.02 24.52 20.02 26.02C20.02 28.16 20 29.88 20 30.42C20 30.84 20.3 31.34 21.1 31.18C27.42 29.06 32 23.06 32 16C32 7.16 24.84 0 16 0V0Z"
          />
        </svg>
        <span className="sr-only">GitHub repository</span>
      </a>
    </footer>
  );
}

type ViewerShellProps = {
  current: string;
  onStop?: () => void;
  navigationLinks?: readonly ViewerNavigationLink[];
  statusIndicator?: ReactNode;
  children: ReactNode;
};

export function ViewerShell({
  current,
  onStop,
  navigationLinks = workspaceNavigationLinks,
  statusIndicator,
  children,
}: ViewerShellProps) {
  return (
    <TooltipProvider>
      <SidebarProvider className="flex-col md:flex-row">
        <ViewerNavigation
          current={current}
          onStop={onStop}
          navigationLinks={navigationLinks}
          statusIndicator={statusIndicator}
        />
        <div className="flex min-w-0 flex-1 flex-col">
          <div className="flex-1">{children}</div>
          <ViewerFooter />
        </div>
      </SidebarProvider>
    </TooltipProvider>
  );
}

function ViewerNavigation({
  current,
  onStop,
  navigationLinks,
  statusIndicator,
}: Pick<ViewerShellProps, "current" | "onStop" | "statusIndicator"> & {
  navigationLinks: readonly ViewerNavigationLink[];
}) {
  const { isMobile, open, openMobile, setOpenMobile } = useSidebar();
  const { stationState, connected } = useStationState();
  const [stopMessage, setStopMessage] = useState("");
  const [stopPending, setStopPending] = useState(false);
  const [stopFailed, setStopFailed] = useState(false);
  useEffect(() => {
    if (!stopMessage || stopPending || stopFailed) return;
    const timeout = window.setTimeout(() => setStopMessage(""), 5000);
    return () => window.clearTimeout(timeout);
  }, [stopMessage, stopPending, stopFailed]);
  async function stopArms(): Promise<void> {
    if (!connected || !stationState || stopPending) return;
    setStopPending(true);
    setStopFailed(false);
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
    setStopFailed(failures.length > 0);
    setStopPending(false);
    setStopMessage(failures.length ? failures.join(" · ") : "Stop requested.");
  }
  const stopDisabled = stopPending || !connected || !stationState?.arms.length;
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
          <StationConnection
            connected={connected}
            label={connected ? "Connected" : "Offline"}
          />
          {statusIndicator}
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
                render={<a href="./#workspace" />}
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
          {!isMobile && (
            <>
              <StationConnection
                connected={connected}
                label={connectionLabel}
              />
              {statusIndicator}
            </>
          )}
          <StopArmsButton
            className="group-data-[collapsible=icon]:size-8 group-data-[collapsible=icon]:p-0"
            disabled={stopDisabled}
            onStop={stopArms}
          />
          {showInlineFeedback && (
            <StopFeedback message={stopMessage} failed={stopFailed} />
          )}
        </SidebarFooter>
      </Sidebar>
      {!showInlineFeedback && (
        <StopFeedback
          className="fixed right-3 bottom-3 z-30 max-w-[min(22rem,calc(100dvw-1.5rem))] shadow-md"
          message={stopMessage}
          failed={stopFailed}
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
      variant="stop"
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
  failed,
}: {
  className?: string;
  message: string;
  failed: boolean;
}) {
  return (
    <div role="status">
      {message && (
        <Alert
          role="presentation"
          variant={failed ? "destructive" : "default"}
          className={cn("max-h-32 overflow-y-auto wrap-anywhere", className)}
        >
          {message}
        </Alert>
      )}
    </div>
  );
}

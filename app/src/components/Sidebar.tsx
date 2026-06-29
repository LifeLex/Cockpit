import type { ViewState } from "../store";
import {
  ListChecks,
  FileText,
  Layers,
  Rocket,
  Settings,
  PanelLeftClose,
  PanelLeftOpen,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

type NavKind = ViewState["kind"];

interface SidebarProps {
  readonly activeView: NavKind;
  readonly reviewCount: number;
  readonly hasPlan: boolean;
  readonly onNavigate: (kind: NavKind) => void;
  readonly collapsed?: boolean | undefined;
  readonly onToggleCollapse?: (() => void) | undefined;
}

interface NavItemProps {
  readonly label: string;
  readonly icon: React.ReactNode;
  readonly active: boolean;
  readonly badge?: string | undefined;
  readonly collapsed?: boolean | undefined;
  readonly onClick: () => void;
}

function NavItem({ label, icon, active, badge, collapsed, onClick }: NavItemProps) {
  const button = (
    <Button
      variant="ghost"
      onClick={onClick}
      className={cn(
        "w-full justify-start gap-3 rounded-md px-3 py-2 text-sm font-medium transition-all duration-200",
        collapsed === true && "justify-center px-2",
        active
          ? "border-l-2 border-primary bg-muted text-foreground"
          : "border-l-2 border-transparent text-muted-foreground hover:bg-muted hover:text-foreground",
      )}
    >
      {icon}
      {collapsed !== true && (
        <>
          <span className="flex-1 text-left">{label}</span>
          {badge !== undefined && (
            <Badge variant="secondary" className="ml-auto text-xs">
              {badge}
            </Badge>
          )}
        </>
      )}
    </Button>
  );

  if (collapsed === true) {
    return (
      <Tooltip>
        <TooltipTrigger render={<div />}>
          {button}
        </TooltipTrigger>
        <TooltipContent side="right" sideOffset={8}>
          {label}
        </TooltipContent>
      </Tooltip>
    );
  }

  return button;
}

export function Sidebar({
  activeView,
  reviewCount,
  hasPlan,
  onNavigate,
  collapsed = false,
  onToggleCollapse,
}: SidebarProps) {
  return (
    <aside
      className={cn(
        "flex shrink-0 flex-col border-r border-border bg-surface-1 transition-all duration-200",
        collapsed ? "w-[60px]" : "w-[240px]",
      )}
    >
      <div
        className={cn(
          "flex items-center py-5 transition-all duration-200",
          collapsed ? "justify-center px-2" : "gap-2 px-4",
        )}
      >
        <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-primary text-sm font-bold text-primary-foreground">
          C
        </div>
        {!collapsed && (
          <span className="flex-1 text-base font-semibold text-foreground">
            Cockpit
          </span>
        )}
        {onToggleCollapse !== undefined && !collapsed && (
          <Button
            variant="ghost"
            size="icon-xs"
            onClick={onToggleCollapse}
            className="text-muted-foreground hover:text-foreground"
          >
            <PanelLeftClose className="h-4 w-4" />
          </Button>
        )}
      </div>

      {onToggleCollapse !== undefined && collapsed && (
        <div className="flex justify-center px-2 pb-2">
          <Tooltip>
            <TooltipTrigger render={<div />}>
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={onToggleCollapse}
                className="text-muted-foreground hover:text-foreground"
              >
                <PanelLeftOpen className="h-4 w-4" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right" sideOffset={8}>
              Expand sidebar
            </TooltipContent>
          </Tooltip>
        </div>
      )}

      <nav
        className={cn(
          "flex flex-1 flex-col gap-1 transition-all duration-200",
          collapsed ? "px-1.5" : "px-3",
        )}
      >
        <NavItem
          label="Reviews"
          icon={<ListChecks className="h-4 w-4 shrink-0" />}
          active={activeView === "frontier" || activeView === "diff"}
          badge={reviewCount > 0 ? String(reviewCount) : undefined}
          collapsed={collapsed}
          onClick={() => {
            onNavigate("frontier");
          }}
        />
        <NavItem
          label="Plan"
          icon={<FileText className="h-4 w-4 shrink-0" />}
          active={activeView === "plan"}
          badge={hasPlan ? "loaded" : undefined}
          collapsed={collapsed}
          onClick={() => {
            onNavigate("plan");
          }}
        />
        <NavItem
          label="Stacks"
          icon={<Layers className="h-4 w-4 shrink-0" />}
          active={activeView === "stacks"}
          collapsed={collapsed}
          onClick={() => {
            onNavigate("stacks");
          }}
        />

        <div className="flex-1" />

        <Separator className="mx-1" />

        <div className="flex flex-col gap-1 py-2">
          <NavItem
            label="Kickoff"
            icon={<Rocket className="h-4 w-4 shrink-0" />}
            active={activeView === "kickoff"}
            collapsed={collapsed}
            onClick={() => {
              onNavigate("kickoff");
            }}
          />
          <NavItem
            label="Settings"
            icon={<Settings className="h-4 w-4 shrink-0" />}
            active={activeView === "settings"}
            collapsed={collapsed}
            onClick={() => {
              onNavigate("settings");
            }}
          />
        </div>
      </nav>
    </aside>
  );
}

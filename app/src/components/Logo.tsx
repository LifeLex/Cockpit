import { cn } from "@/lib/utils";

interface LogoProps {
  readonly size?: number | undefined;
  readonly className?: string | undefined;
}

export function Logo({ size = 28, className }: LogoProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 32 32"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      className={cn("shrink-0", className)}
    >
      {/* Outer rounded square */}
      <rect
        x="1"
        y="1"
        width="30"
        height="30"
        rx="8"
        className="fill-primary"
      />
      {/* Gauge/cockpit dial — arc + needle */}
      <path
        d="M8 22a8 8 0 0 1 16 0"
        className="stroke-primary-foreground"
        strokeWidth="2.5"
        strokeLinecap="round"
        fill="none"
      />
      {/* Needle pointing upper-right */}
      <line
        x1="16"
        y1="22"
        x2="21"
        y2="14"
        className="stroke-primary-foreground"
        strokeWidth="2.5"
        strokeLinecap="round"
      />
      {/* Center dot */}
      <circle cx="16" cy="22" r="2" className="fill-primary-foreground" />
    </svg>
  );
}

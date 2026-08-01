import type { ReactNode } from "react";

type IconProps = {
  children: ReactNode;
};

function Icon({ children }: IconProps) {
  return (
    <svg
      aria-hidden="true"
      className="app-icon"
      fill="none"
      viewBox="0 0 24 24"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {children}
    </svg>
  );
}

export function PlusIcon() {
  return <Icon><path d="M12 5v14M5 12h14" /></Icon>;
}

export function SearchIcon() {
  return <Icon><circle cx="11" cy="11" r="6" /><path d="m16 16 4 4" /></Icon>;
}

export function FolderIcon() {
  return <Icon><path d="M3 7.5h7l2 2h9v9.5H3z" /><path d="M3 7.5v-2h7l2 2" /></Icon>;
}

export function SunIcon() {
  return <Icon><circle cx="12" cy="12" r="3.5" /><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" /></Icon>;
}

export function MoonIcon() {
  return <Icon><path d="M20 15.3A8.5 8.5 0 0 1 8.7 4 8.5 8.5 0 1 0 20 15.3Z" /></Icon>;
}

export function CloseIcon() {
  return <Icon><path d="m6 6 12 12M18 6 6 18" /></Icon>;
}

export function AgentIcon() {
  return <Icon><path d="M7 8.5 12 5l5 3.5v7L12 19l-5-3.5z" /><path d="m9.5 11 2.5 1.5 2.5-1.5M12 12.5V16" /></Icon>;
}

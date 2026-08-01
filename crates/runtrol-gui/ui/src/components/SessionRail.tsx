import {
  Badge,
  Button,
  SideNav,
  SideNavHeading,
  SideNavItem,
  SideNavSection,
  StatusDot,
  TextInput,
} from "@astryxdesign/core";
import type { SessionRow, ThemeMode } from "../domain";
import { FolderIcon, MoonIcon, PlusIcon, SearchIcon, SunIcon } from "../icons";

type SessionRailProps = {
  rows: readonly SessionRow[];
  selected: string | null;
  query: string;
  reachable: boolean;
  theme: ThemeMode;
  onQueryChange: (value: string) => void;
  onSelect: (session: string) => void;
  onStart: () => void;
  onToggleTheme: () => void;
};

function statusOf(row: SessionRow): { variant: "success" | "warning" | "neutral"; label: string } {
  if (row.looksStuck) {
    return { variant: "warning", label: `${row.doing}, 응답이 없다` };
  }
  if (row.hot) {
    return { variant: "success", label: row.doing };
  }
  return { variant: "neutral", label: row.doing };
}

function shortName(row: SessionRow): string {
  return (row.native ?? row.session).slice(0, row.native ? 22 : 8);
}

export function SessionRail({
  rows,
  selected,
  query,
  reachable,
  theme,
  onQueryChange,
  onSelect,
  onStart,
  onToggleTheme,
}: SessionRailProps) {
  const needle = query.trim().toLocaleLowerCase();
  const visible = needle
    ? rows.filter((row) =>
        [row.folder, row.workspace, row.provider, row.native ?? "", row.session]
          .some((value) => value.toLocaleLowerCase().includes(needle)),
      )
    : rows;
  const groups = new Map<string, SessionRow[]>();
  for (const row of visible) {
    const group = groups.get(row.workspace) ?? [];
    group.push(row);
    groups.set(row.workspace, group);
  }

  return (
    <SideNav
      aria-label="세션 탐색"
      className="session-rail"
      header={
        <SideNavHeading
          heading="runtrol"
          subheading="에이전트 세션"
          icon={<span className="brand-symbol">r</span>}
        />
      }
      topContent={
        <div className="rail-actions">
          <Button
            label="새 세션"
            variant="secondary"
            icon={<PlusIcon />}
            onClick={onStart}
          />
          <TextInput
            label="세션 검색"
            isLabelHidden
            value={query}
            onChange={onQueryChange}
            placeholder="세션 검색"
            startIcon={<SearchIcon />}
            hasClear
            width="100%"
            size="sm"
          />
        </div>
      }
      footer={
        <div className="rail-footer">
          <div className="daemon-state">
            <StatusDot
              variant={reachable ? "success" : "error"}
              label={reachable ? "데몬 연결됨" : "데몬에 닿지 않는다"}
            />
            <span>{reachable ? "데몬 연결됨" : "데몬에 닿지 않는다"}</span>
          </div>
          <Button
            label={theme === "dark" ? "밝은 테마" : "어두운 테마"}
            tooltip={theme === "dark" ? "밝은 테마" : "어두운 테마"}
            variant="ghost"
            size="sm"
            isIconOnly
            icon={theme === "dark" ? <SunIcon /> : <MoonIcon />}
            onClick={onToggleTheme}
          />
        </div>
      }
      resizable={{ defaultWidth: 276, minWidth: 228, maxWidth: 380, autoSaveId: "runtrol-session-rail" }}
      collapsible={{ buttonLabel: "세션 탐색 접기" }}
    >
      {groups.size === 0 ? (
        <div className="rail-empty">
          {rows.length === 0 ? "아직 세션이 없다." : "검색 결과가 없다."}
        </div>
      ) : (
        [...groups.entries()].map(([workspace, sessions]) => (
          <SideNavSection
            key={workspace}
            title={sessions[0].folder || workspace}
            subtitle={workspace}
          >
            {sessions.map((row) => {
              const status = statusOf(row);
              return (
                <SideNavItem
                  key={row.session}
                  label={shortName(row)}
                  icon={<FolderIcon />}
                  isSelected={row.session === selected}
                  onClick={() => onSelect(row.session)}
                  endContent={
                    <span className="row-end" title={status.label}>
                      <StatusDot variant={status.variant} label={status.label} isPulsing={row.hot && row.doing !== "idle"} />
                      <Badge label={row.provider} variant="neutral" />
                    </span>
                  }
                  data-testid={`session-${row.session}`}
                />
              );
            })}
          </SideNavSection>
        ))
      )}
    </SideNav>
  );
}

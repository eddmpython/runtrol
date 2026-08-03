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
import { memo } from "react";
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
  onConsult: () => void;
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

/** How much of an identifier is shown when it is too long to show whole. */
const NAME_BUDGET = 14;

/**
 * A session's name, short enough for the rail and still able to tell two sessions apart.
 *
 * Kept from the end. Both identifiers here are UUIDv7, whose leading characters are a timestamp, so
 * sessions started in the same minute share them: measured on this machine, three sessions in two folders
 * all rendered as `019fc4fc…`, which is a label that names nothing. The end is the random part.
 *
 * A provider that names conversations in words keeps them whole, because a name a person chose beats any
 * fragment of an identifier.
 */
function shortName(row: SessionRow): string {
  const name = row.native ?? row.session;
  if (name.length <= NAME_BUDGET) {
    return name;
  }
  return `…${name.slice(-NAME_BUDGET)}`;
}

export const SessionRail = memo(function SessionRail({
  rows,
  selected,
  query,
  reachable,
  theme,
  onQueryChange,
  onSelect,
  onStart,
  onConsult,
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
        // No mark here. The window's own title bar already carries the real one, and a second brand
        // block inside a 276px rail is chrome that costs list space and says nothing new. What was here
        // was not even the mark: it was an orange `r` drawn in CSS, while the real symbol is the four
        // corner brackets in `assets/brand/symbol.svg`.
        <SideNavHeading heading="runtrol" subheading="에이전트 세션" />
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
            label="AI 자문"
            tooltip="한 AI가 턴 중에 다른 AI의 의견을 받아오게 연결한다"
            variant="ghost"
            size="sm"
            onClick={onConsult}
            data-testid="open-consult"
          />
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
      resizable={{ defaultWidth: 276, minWidth: 228, maxWidth: 380 }}
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
            // The shortened form, which keeps the end of the path. The whole path is still one hover
            // away on each row, and the head is what every path on a machine has in common.
            subtitle={sessions[0].trail}
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
                    // The whole identifier and the whole path ride the hover, for the one moment somebody
                    // needs to read or copy them. Neither belongs on the line itself at this width, and the
                    // item component takes no title of its own.
                    <span
                      className="row-end"
                      title={`${status.label}\n${row.native ?? row.session}\n${row.workspace}`}
                    >
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
});

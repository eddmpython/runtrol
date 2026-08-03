import {
  Badge,
  Button,
  Dialog,
  DialogHeader,
  Layout,
  LayoutContent,
  StatusDot,
} from "@astryxdesign/core";
import type { ConsultDirection, OfferedProvider } from "../domain";

type ConsultDialogProps = {
  isOpen: boolean;
  directions: readonly ConsultDirection[];
  providers: readonly OfferedProvider[];
  busy: string | null;
  loading: boolean;
  onOpenChange: (open: boolean) => void;
  onToggle: (direction: ConsultDirection) => void;
};

function keyOf(direction: ConsultDirection): string {
  return `${direction.from}->${direction.to}`;
}

export function ConsultDialog({
  isOpen,
  directions,
  providers,
  busy,
  loading,
  onOpenChange,
  onToggle,
}: ConsultDialogProps) {
  const nameOf = (id: string) =>
    providers.find((entry) => entry.id === id)?.displayName ?? id;

  return (
    <Dialog isOpen={isOpen} onOpenChange={onOpenChange} purpose="form" width={520}>
      <Layout
        height="auto"
        header={
          <DialogHeader
            title="AI 자문 연결"
            subtitle="켜면 한 AI가 턴 중에 다른 AI의 의견을 직접 받아옵니다. 대화는 두 CLI 사이에서만 오갑니다."
            onOpenChange={onOpenChange}
            hasDivider
          />
        }
        content={
          <LayoutContent>
            <div className="consult-directions">
              {loading && directions.length === 0 ? (
                <p className="consult-note">연결 상태를 CLI 설정에서 확인하는 중이다.</p>
              ) : directions.length === 0 ? (
                <p className="consult-note">연결할 수 있는 방향이 없다.</p>
              ) : (
                directions.map((direction) => {
                  const key = keyOf(direction);
                  const wired = direction.state === "wired";
                  const unsupported = direction.state === "unsupported";
                  return (
                    <div className="consult-row" key={key} data-testid={`consult-${key}`}>
                      <div className="consult-row-main">
                        <StatusDot
                          variant={wired ? "success" : unsupported ? "warning" : "neutral"}
                          label={direction.state}
                        />
                        <span className="consult-label">
                          {nameOf(direction.from)} 가 {nameOf(direction.to)} 의 의견을 듣는다
                        </span>
                        {unsupported ? (
                          <Badge label="지원 안 됨" variant="neutral" />
                        ) : (
                          <Button
                            label={wired ? "끄기" : "켜기"}
                            variant={wired ? "secondary" : "primary"}
                            size="sm"
                            isLoading={busy === key}
                            isDisabled={busy !== null}
                            onClick={() => onToggle(direction)}
                            data-testid={`consult-toggle-${key}`}
                          />
                        )}
                      </div>
                      {unsupported && direction.why ? (
                        <p className="consult-note">{direction.why}</p>
                      ) : null}
                    </div>
                  );
                })
              )}
            </div>
          </LayoutContent>
        }
      />
    </Dialog>
  );
}

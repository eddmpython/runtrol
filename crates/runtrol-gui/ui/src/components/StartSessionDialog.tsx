import {
  Button,
  Dialog,
  DialogHeader,
  Layout,
  LayoutContent,
  LayoutFooter,
  Selector,
  TextInput,
} from "@astryxdesign/core";
import type { ModelCatalog, OfferedProvider } from "../domain";

const PROVIDER_DEFAULT = "__provider_default__";

type StartSessionDialogProps = {
  isOpen: boolean;
  providers: readonly OfferedProvider[];
  provider: string;
  model: string;
  models: ModelCatalog | null;
  modelsLoading: boolean;
  workspace: string;
  starting: boolean;
  onOpenChange: (open: boolean) => void;
  onProviderChange: (value: string) => void;
  onModelChange: (value: string) => void;
  onWorkspaceChange: (value: string) => void;
  onStart: () => void;
};

export function StartSessionDialog({
  isOpen,
  providers,
  provider,
  model,
  models,
  modelsLoading,
  workspace,
  starting,
  onOpenChange,
  onProviderChange,
  onModelChange,
  onWorkspaceChange,
  onStart,
}: StartSessionDialogProps) {
  const options = providers.map((entry) => ({
    value: entry.id,
    label: entry.usable
      ? entry.displayName
      : `${entry.displayName} (${entry.whyNot ?? "사용 불가"})`,
    disabled: !entry.usable,
  }));
  const usable = providers.some((entry) => entry.usable);
  const modelOptions = [
    { value: PROVIDER_DEFAULT, label: "공급자 기본값" },
    ...(models?.kind === "known"
      ? models.models.map((entry) => ({
          value: entry.id,
          label: entry.isDefault ? `${entry.displayName} (기본)` : entry.displayName,
        }))
      : models?.kind === "aliases"
        ? models.aliases.map((alias) => ({ value: alias, label: alias }))
        : []),
  ];
  const modelNote = modelsLoading
    ? "CLI에서 현재 모델 정보를 확인하는 중이다."
    : models?.kind === "aliases"
      ? models.why
      : models?.kind === "unknown"
        ? models.why
        : models?.kind === "known" && models.models.length === 0
          ? "CLI가 현재 선택 가능한 모델을 보고하지 않았다."
          : null;

  return (
    <Dialog isOpen={isOpen} onOpenChange={onOpenChange} purpose="form" width={480}>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          onStart();
        }}
      >
        <Layout
          height="auto"
          header={
            <DialogHeader
              title="새 세션"
              subtitle="발견된 CLI와 작업 폴더를 선택합니다."
              onOpenChange={onOpenChange}
              hasDivider
            />
          }
          content={
            <LayoutContent>
              <div className="start-fields">
                <Selector
                  label="공급자"
                  options={options}
                  value={provider || undefined}
                  onChange={onProviderChange}
                  placeholder={providers.length === 0 ? "발견된 공급자가 없다" : "공급자 선택"}
                  isDisabled={!usable}
                  disabledMessage={!usable ? "사용 가능한 공급자 CLI가 없다" : undefined}
                  width="100%"
                />
                <Selector
                  label="모델"
                  options={modelOptions}
                  value={model || PROVIDER_DEFAULT}
                  onChange={(value) => onModelChange(value === PROVIDER_DEFAULT ? "" : value)}
                  isDisabled={!provider || modelsLoading || models?.kind === "unknown"}
                  disabledMessage={models?.kind === "unknown" ? modelNote ?? undefined : undefined}
                  width="100%"
                />
                {modelNote ? <p className="model-note">{modelNote}</p> : null}
                <TextInput
                  label="작업 폴더"
                  value={workspace}
                  onChange={onWorkspaceChange}
                  placeholder="C:\\work\\project"
                  isRequired
                  width="100%"
                  hasAutoFocus
                />
              </div>
            </LayoutContent>
          }
          footer={
            <LayoutFooter hasDivider>
              <div className="dialog-actions">
                <Button label="취소" variant="ghost" onClick={() => onOpenChange(false)} />
                <Button
                  label="시작"
                  variant="primary"
                  type="submit"
                  isLoading={starting}
                  isDisabled={!provider || !workspace.trim() || starting}
                />
              </div>
            </LayoutFooter>
          }
        />
      </form>
    </Dialog>
  );
}

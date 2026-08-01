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
import type { OfferedProvider } from "../domain";

type StartSessionDialogProps = {
  isOpen: boolean;
  providers: readonly OfferedProvider[];
  provider: string;
  workspace: string;
  starting: boolean;
  onOpenChange: (open: boolean) => void;
  onProviderChange: (value: string) => void;
  onWorkspaceChange: (value: string) => void;
  onStart: () => void;
};

export function StartSessionDialog({
  isOpen,
  providers,
  provider,
  workspace,
  starting,
  onOpenChange,
  onProviderChange,
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

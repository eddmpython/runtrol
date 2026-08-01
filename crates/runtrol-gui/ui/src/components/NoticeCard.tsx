import { Button, Text } from "@astryxdesign/core";
import type { Notice } from "../domain";
import { CloseIcon } from "../icons";

export function NoticeCard({ notice, onClose }: { notice: Notice; onClose: () => void }) {
  const label = notice.kind === "warning"
    ? "저장소 경고"
    : notice.kind === "refused"
      ? "요청 거절"
      : "연결 문제";
  return (
    <div className="notice-card" data-kind={notice.kind} role="status">
      <div>
        <Text type="label" display="block">{label}</Text>
        <Text type="supporting" display="block">{notice.message}</Text>
      </div>
      <Button
        label="알림 닫기"
        tooltip="알림 닫기"
        variant="ghost"
        size="sm"
        isIconOnly
        icon={<CloseIcon />}
        onClick={onClose}
      />
    </div>
  );
}

//! 星形图标（收藏态共用组件）：实心 = 已收藏，描边 = 未收藏；颜色继承 currentColor。
//!
//! 视觉约定（契约 `docs/api/clipboard-history.md` 5.8）：
//! - 主窗口：未收藏 text-tertiary 描边、已收藏 accent 实心；
//! - 小屏选中行：反色为 accent-text（实心/描边区分状态）。

export function StarIcon(props: { filled: boolean }) {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true">
      <path
        d="M12 2.6l2.86 5.94 6.54.86-4.79 4.53 1.19 6.47L12 17.2l-5.8 3.2 1.19-6.47L2.6 9.4l6.54-.86L12 2.6z"
        fill={props.filled ? "currentColor" : "none"}
        stroke="currentColor"
        stroke-width="1.7"
        stroke-linejoin="round"
      />
    </svg>
  );
}

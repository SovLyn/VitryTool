//! 全局通知容器（0.2.8）：右上角 toast 堆栈，仅主窗口挂载。
//!
//! 职责（契约 `docs/api/notify.md` 5.3/5.5）：
//! - 监听 `app://notify` 事件，将通知渲染为右上角竖排 toast；
//! - 分 level 自动消失（success 3s / info 4s / warning 6s / error 8s），hover 暂停计时；
//! - warning / error 带手动关闭按钮；success / info 无；
//! - 容量最多 4 条，新到顶部、旧的向下让位；同 level+code 3 秒内重复到达去重置顶计时；
//! - 渲染时按当前 locale 翻译（切语言即时重译）；`role="status"` + `aria-live="polite"`；
//! - 动效：进入材质化（translateY + scale + opacity + blur），退出对称上滑淡出；
//!   `prefers-reduced-motion` 时降级为纯 opacity 交叉淡化（CSS 媒体查询）。
//!
//! 小屏（quick-paste popup）不挂载本组件（瞬态窗口，简略为要，契约 5.6）。
//!
//! 防频闪设计（0.2.8 实测修复）：
//! - toast 持**到期时间戳**（`deadline`），tick 只做「到期检查」，未到期时**完全不调用
//!   setToasts**——`<For>` 按对象引用 keyed，引用不变则 DOM 不重建、CSS 进入动画不重放
//!   （否则每 200ms 全列表重建一次 = 每 tick 重播进入动画 = 可见频闪）；
//! - hover 暂停用**累计暂停偏移**（`pausedMs`）实现：暂停/恢复不触碰 toast 状态，只在
//!   tick 检查时把偏移加进 deadline——暂停期间到期时刻顺延，恢复不触发任何渲染。

import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { listen } from "@tauri-apps/api/event";
import {
  APP_NOTIFY_EVENT,
  resolveNotifyCode,
  UNKNOWN_NOTIFY_CODE,
  type NotifyLevel,
  type NotifyPayload,
} from "../api/notify";
import { useI18n } from "../i18n";

/** 各 level 自动消失时长（ms，契约 5.3）。 */
export const LEVEL_DURATION: Record<NotifyLevel, number> = {
  success: 3000,
  info: 4000,
  warning: 6000,
  error: 8000,
};

/** 同 level+code 去重窗口（ms，契约 5.5）。 */
export const DEDUP_WINDOW_MS = 3000;

/** 同时最多显示条数（契约 5.5）。 */
export const MAX_TOASTS = 4;

/** 退出动画时长（ms，与 CSS 过渡一致）。 */
export const EXIT_MS = 260;

/** 计时器 tick（ms）。 */
export const TICK_MS = 200;

interface ToastItem {
  id: number;
  level: NotifyLevel;
  code: string;
  params?: Record<string, string | number | boolean>;
  /** 到期时间戳（ms，epoch）；hover 暂停时经 `pausedMs` 顺延。 */
  deadline: number;
  /** 是否处于退出动画中（从数组中移除前短暂保留）。 */
  leaving: boolean;
}

let nextId = 1;

export function NotificationProvider() {
  const { t } = useI18n();
  const [toasts, setToasts] = createSignal<ToastItem[]>([]);
  const [paused, setPaused] = createSignal(false);
  /** 累计暂停毫秒（hover 期间到期时刻顺延；tick 检查时加到 deadline 上）。 */
  let pausedMs = 0;
  /** 本次暂停起点（hover 进入时刻）。 */
  let pauseStart = 0;
  const itemRefs = new Map<number, HTMLElement>();
  let prevPositions = new Map<number, number>();

  /** 当前时间（tick/去重检查用，测试可注入 fake timers）。 */
  function now(): number {
    return Date.now();
  }

  /** 将通知加入堆栈（新到顶部；超容量立即淘汰最旧；去重命中则重置该条计时）。 */
  function push(payload: NotifyPayload) {
    const current = now();
    setToasts((prev) => {
      // 去重：同 level+code 且在去重窗口内 → 不新增，重置该条到期时刻（契约 5.5）
      const dup = prev.find(
        (toast) =>
          !toast.leaving &&
          toast.level === payload.level &&
          toast.code === payload.code &&
          toast.deadline + pausedMs > current + LEVEL_DURATION[payload.level] - DEDUP_WINDOW_MS,
      );
      if (dup) {
        return prev.map((toast) =>
          toast.id === dup.id
            ? { ...toast, deadline: current + LEVEL_DURATION[payload.level] }
            : toast,
        );
      }
      const item: ToastItem = {
        id: nextId++,
        level: payload.level,
        code: payload.code,
        params: payload.params,
        deadline: current + LEVEL_DURATION[payload.level],
        leaving: false,
      };
      // 新到顶部；超容量淘汰最旧（不排队，立即消失）
      const next = [item, ...prev];
      return next.length > MAX_TOASTS ? next.slice(0, MAX_TOASTS) : next;
    });
  }

  /** 标记一条 toast 进入退出动画，动画结束后移出数组（幂等）。 */
  function markLeaving(id: number) {
    setToasts((prev) => {
      const target = prev.find((toast) => toast.id === id);
      if (!target || target.leaving) return prev;
      return prev.map((toast) => (toast.id === id ? { ...toast, leaving: true } : toast));
    });
    window.setTimeout(() => {
      setToasts((prev) => prev.filter((toast) => toast.id !== id));
    }, EXIT_MS);
  }

  onMount(() => {
    const unlisten = listen<NotifyPayload>(APP_NOTIFY_EVENT, (event) => {
      push(event.payload);
    });

    // 计时器：仅检查「到期时间戳」，未到期完全不 setToasts（防频闪，见模块注释）。
    const timer = window.setInterval(() => {
      const current = now();
      const offset = paused() ? pausedMs + (current - pauseStart) : pausedMs;
      const expired = toasts()
        .filter((toast) => !toast.leaving && toast.deadline + offset <= current)
        .map((toast) => toast.id);
      if (expired.length === 0) return; // 未到期：不触发任何更新
      setToasts((prev) =>
        prev.map((toast) => (expired.includes(toast.id) ? { ...toast, leaving: true } : toast)),
      );
      // 动画结束后移出数组
      for (const id of expired) {
        window.setTimeout(() => {
          setToasts((cur) => cur.filter((item) => item.id !== id));
        }, EXIT_MS);
      }
    }, TICK_MS);

    onCleanup(() => {
      void unlisten.then((fn) => fn());
      window.clearInterval(timer);
    });
  });

  // FLIP 让位：新 toast 插入顶部后，旧 toast 平滑下移（弹簧近似：cubic-bezier 0.22/1/0.36/1）
  createEffect(() => {
    const list = toasts();
    const raf = window.requestAnimationFrame(() => {
      const nextPositions = new Map<number, number>();
      for (const toast of list) {
        const el = itemRefs.get(toast.id);
        if (el) nextPositions.set(toast.id, el.offsetTop);
      }
      for (const [id, el] of itemRefs) {
        const prev = prevPositions.get(id);
        const next = nextPositions.get(id);
        if (prev !== undefined && next !== undefined && prev !== next) {
          const delta = prev - next;
          el.style.transition = "none";
          el.style.transform = `translateY(${delta}px)`;
          window.requestAnimationFrame(() => {
            el.style.transition = "transform 0.4s cubic-bezier(0.22, 1, 0.36, 1)";
            el.style.transform = "translateY(0)";
          });
        }
      }
      prevPositions = nextPositions;
    });
    onCleanup(() => window.cancelAnimationFrame(raf));
  });

  return (
    <div
      class="toast-stack"
      role="status"
      aria-live="polite"
      onMouseEnter={() => {
        if (!paused()) {
          pauseStart = now();
          setPaused(true);
        }
      }}
      onMouseLeave={() => {
        if (paused()) {
          pausedMs += now() - pauseStart;
          setPaused(false);
        }
      }}
    >
      <For each={toasts()}>
        {(toast) => {
          const i18nKey = resolveNotifyCode(toast.code);
          const text = () =>
            t(i18nKey, toast.params) || t(UNKNOWN_NOTIFY_CODE, { code: toast.code });
          return (
            <div
              class={
                toast.leaving ? `toast toast-${toast.level} leaving` : `toast toast-${toast.level}`
              }
              ref={(el) => itemRefs.set(toast.id, el)}
            >
              <span class="toast-bar" aria-hidden="true" />
              <span class="toast-dot" aria-hidden="true" />
              <span class="toast-text">{text()}</span>
              <Show when={toast.level === "warning" || toast.level === "error"}>
                <button
                  type="button"
                  class="toast-dismiss"
                  aria-label={t("notify.dismiss")}
                  onClick={() => markLeaving(toast.id)}
                >
                  ✕
                </button>
              </Show>
            </div>
          );
        }}
      </For>
    </div>
  );
}

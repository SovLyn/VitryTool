# 接口契约文档模板

> 用法：复制本文件为 `docs/api/<feature>.md`，填写以下内容。**在写任何实现代码之前完成本文件**。

## 功能名：<功能名>

- 状态：`草案` | `已实现` | `已废弃`
- 关联功能文档：[docs/features/<feature>.md](../features/<feature>.md)
- 版本影响：`patch` | `minor` | `major`

## 1. 概述

<这个功能要解决什么问题，一段话。>

## 2. 命令列表

| 命令 | 方向 | 说明 |
| --- | --- | --- |
| `<commandName>` | 前端 → 后端 | <一句话说明> |

## 3. 类型定义

### 请求（前端 → 后端）

```ts
// TypeScript（前端视角）
interface <ReqName> {
  // 字段名: 类型,  // 说明（必填/可选，单位）
}
```

```rust
// Rust（后端视角，serde 反序列化）
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct <ReqName> {
    // 字段与上方 TS 一一对应
}
```

### 响应（后端 → 前端）

```ts
interface <RespName> {
  // 字段名: 类型,  // 说明
}
```

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct <RespName> {
    // 字段与上方 TS 一一对应
}
```

## 4. 错误码

| 错误码 | 含义 | 中文文案建议 | 英文文案建议 |
| --- | --- | --- | --- |
| `<domain>.<error>` | <说明> | <中文> | <English> |

## 5. 行为说明

<关键业务规则、边界条件、时序/数据流描述。>

## 6. 破坏性影响

<该接口变更是否破坏现有前端/后端？迁移路径是什么？>

## 7. 未决问题

- <需要讨论确认的点>

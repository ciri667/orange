/**
 * 内置斜杠指令的跨端注册表。
 *
 * 这里是所有内置指令的唯一真相源：指令名、别名、说明、可用端和参数提示都只在此声明，
 * 桌面端 `/` 弹窗、`/help` 文案和 IM 指令解析全部从同一份列表取数，
 * 避免出现「帮助文案宣传了某条指令、但解析表里没有」这类漂移。
 */
use serde::Serialize;

/** 指令可用的入口。同一条指令可以同时服务桌面端和 IM。 */
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandScope {
    Desktop,
    Im,
}

/** 内置指令描述符；只包含可公开给前端的元数据，不含任何处理函数或内部路径。 */
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinCommandDescriptor {
    /** 稳定的指令 ID，用于前后端分派和埋点，不随文案调整变化。 */
    pub id: &'static str,
    /** 主命令名，不含前导斜杠。 */
    pub name: &'static str,
    /** 兼容别名；解析时与主名等价，弹窗中不重复展示。 */
    pub aliases: &'static [&'static str],
    /** 一行说明，同时用于 `/help` 与 `/` 弹窗。 */
    pub description: &'static str,
    /** 参数占位提示；空串表示该指令不接受参数。 */
    pub argument_hint: &'static str,
    /** 该指令在哪些端可用。 */
    pub scopes: &'static [CommandScope],
    /** 是否必须有活跃的持久化会话；草稿会话下应提示用户先发送一条消息。 */
    pub requires_active_session: bool,
}

/**
 * 全部内置指令。数组顺序即 `/` 弹窗与 `/help` 的展示顺序：
 * 常用的会话与信息指令在前，不按字母排序。
 */
pub fn builtin_commands() -> &'static [BuiltinCommandDescriptor] {
    &[
        BuiltinCommandDescriptor {
            id: "help",
            name: "help",
            aliases: &[],
            description: "查看全部内置指令说明",
            argument_hint: "",
            scopes: &[CommandScope::Desktop, CommandScope::Im],
            requires_active_session: false,
        },
        BuiltinCommandDescriptor {
            id: "status",
            name: "status",
            aliases: &[],
            description: "查看当前会话、模型与上下文状态",
            argument_hint: "",
            scopes: &[CommandScope::Desktop, CommandScope::Im],
            requires_active_session: false,
        },
        BuiltinCommandDescriptor {
            id: "new",
            name: "new",
            aliases: &["reset", "clear"],
            description: "开启新的 Agent 会话",
            argument_hint: "",
            scopes: &[CommandScope::Desktop, CommandScope::Im],
            requires_active_session: false,
        },
        BuiltinCommandDescriptor {
            id: "compact",
            name: "compact",
            aliases: &[],
            description: "整理当前会话上下文",
            argument_hint: "",
            scopes: &[CommandScope::Desktop, CommandScope::Im],
            requires_active_session: true,
        },
        BuiltinCommandDescriptor {
            id: "rename",
            name: "rename",
            aliases: &["title"],
            description: "重命名当前会话",
            argument_hint: "<标题>",
            scopes: &[CommandScope::Desktop],
            requires_active_session: true,
        },
    ]
}

/** 按 ID 查找指令描述符；ID 由注册表自身维护，写入时即保证唯一。 */
pub fn find_builtin_command(command_id: &str) -> Option<&'static BuiltinCommandDescriptor> {
    builtin_commands()
        .iter()
        .find(|command| command.id == command_id)
}

/** 只保留指定端可用的指令，供前端 `/` 弹窗和 IM 解析各自取数。 */
pub fn builtin_commands_for_scope(scope: CommandScope) -> Vec<&'static BuiltinCommandDescriptor> {
    builtin_commands()
        .iter()
        .filter(|command| command.scopes.contains(&scope))
        .collect()
}

/** 判断某个 token 是否命中该指令的主名或任一别名。 */
fn matches_token(command: &BuiltinCommandDescriptor, token: &str) -> bool {
    command.name == token || command.aliases.contains(&token)
}

/**
 * 解析命令行文本；命中时返回指令描述符和去掉前导空白后的参数原文。
 *
 * 规则与主流 Agent CLI 对齐：整行必须以 `/` 开头，命令名取第一个空白前的片段，
 * 其余内容原样作为 args 交给指令自己解释，不做结构化 flag 解析。
 * 未知指令返回 None，调用方应降级为普通消息而不是报错。
 */
pub fn parse_builtin_command(text: &str) -> Option<(&'static BuiltinCommandDescriptor, String)> {
    let trimmed = text.trim_start();
    let body = trimmed.strip_prefix('/')?;

    // 命令名只取第一个空白前的片段，因此 `/help more` 也能命中 `/help` 并把 more 作为参数。
    let name_end = body
        .find(|character: char| character.is_whitespace())
        .unwrap_or(body.len());
    let (name, rest) = body.split_at(name_end);

    if name.is_empty() {
        return None;
    }

    // `/usr/local/bin` 这类路径不应被当成指令，只有注册表里的名字才算命中。
    let command = builtin_commands()
        .iter()
        .find(|command| matches_token(command, &name.to_ascii_lowercase()))?;

    Some((command, rest.trim_start().to_owned()))
}

/**
 * 由注册表动态拼接 `/help` 文案。
 *
 * 说明文本不再手写，确保 `/help` 列出的每条指令都真实存在于解析表中；
 * include_im_sections 为真时额外说明 IM 侧的待确认变更文字指令。
 */
pub fn builtin_command_help_text(scope: CommandScope) -> String {
    let mut text = String::from("内置指令：\n");

    for command in builtin_commands_for_scope(scope) {
        let hint = if command.argument_hint.is_empty() {
            String::new()
        } else {
            format!(" {}", command.argument_hint)
        };
        text.push_str(&format!(
            "/{}{}：{}\n",
            command.name, hint, command.description
        ));
    }

    if scope == CommandScope::Im {
        text.push_str("\n待确认变更：发送“详情 <编号>”、“确认 <编号>”或“取消 <编号>”。");
    }

    text
}

#[cfg(test)]
mod tests {
    use super::{
        builtin_command_help_text, builtin_commands, builtin_commands_for_scope,
        find_builtin_command, parse_builtin_command, CommandScope,
    };

    /** 注册表中的 ID 与命令名必须唯一，否则前后端分派会命中不确定的指令。 */
    #[test]
    fn builtin_command_ids_and_names_are_unique() {
        let commands = builtin_commands();
        let mut ids = commands
            .iter()
            .map(|command| command.id)
            .collect::<Vec<_>>();
        let mut names = commands
            .iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();

        ids.sort_unstable();
        ids.dedup();
        names.sort_unstable();
        names.dedup();

        assert_eq!(ids.len(), commands.len());
        assert_eq!(names.len(), commands.len());
    }

    /** 主名与别名都不得互相冲突，且必须全部能被解析器命中。 */
    #[test]
    fn builtin_command_names_and_aliases_all_resolve() {
        for command in builtin_commands() {
            assert_eq!(
                parse_builtin_command(&format!("/{}", command.name)).map(|(parsed, _)| parsed.id),
                Some(command.id)
            );

            for alias in command.aliases {
                assert_eq!(
                    parse_builtin_command(&format!("/{alias}")).map(|(parsed, _)| parsed.id),
                    Some(command.id),
                    "别名 {alias} 应解析为 {}",
                    command.id
                );
            }
        }
    }

    /** 解析器要接受前后空白，并把第一个分隔空白之后的全部内容原样作为参数返回。 */
    #[test]
    fn parses_command_with_args_and_trims_edges() {
        assert_eq!(
            parse_builtin_command(" /rename  我的标题 ").map(|(command, args)| (command.id, args)),
            Some(("rename", "我的标题 ".to_owned()))
        );
        assert_eq!(
            parse_builtin_command("/help").map(|(command, args)| (command.id, args)),
            Some(("help", String::new()))
        );
        assert_eq!(
            parse_builtin_command("/reset").map(|(command, _)| command.id),
            Some("new")
        );
    }

    /** 路径形态与未知指令都必须返回 None，交给调用方降级为普通消息。 */
    #[test]
    fn rejects_paths_and_unknown_commands() {
        assert!(parse_builtin_command("/usr/local/bin").is_none());
        assert!(parse_builtin_command("/unknown").is_none());
        assert!(parse_builtin_command("/").is_none());
        assert!(parse_builtin_command("普通正文 /help").is_none());
    }

    /** 端过滤必须只返回该端声明的指令。 */
    #[test]
    fn scope_filter_returns_only_declared_commands() {
        let desktop = builtin_commands_for_scope(CommandScope::Desktop);
        let im = builtin_commands_for_scope(CommandScope::Im);

        assert!(desktop.iter().any(|command| command.id == "rename"));
        assert!(!im.iter().any(|command| command.id == "rename"));
        assert!(im.iter().any(|command| command.id == "compact"));
    }

    /** 这是防止「帮助文案与解析表漂移」复发的护栏：文案里出现的每条指令都必须可解析。 */
    #[test]
    fn help_text_only_lists_parseable_commands() {
        let help = builtin_command_help_text(CommandScope::Desktop);
        let listed = help
            .lines()
            .filter_map(|line| line.strip_prefix('/'))
            .filter_map(|line| line.split(['：', ' ']).next())
            .collect::<Vec<_>>();

        assert!(!listed.is_empty());
        for name in listed {
            assert!(
                parse_builtin_command(&format!("/{name}")).is_some(),
                "帮助文案列出了 /{name}，但解析器无法命中"
            );
        }

        let im_help = builtin_command_help_text(CommandScope::Im);
        assert!(im_help.contains("/status"));
        assert!(!im_help.contains("/rename"));
    }

    /** find_builtin_command 是命令层分派入口，必须按 ID 精确命中。 */
    #[test]
    fn find_builtin_command_resolves_by_id() {
        assert_eq!(
            find_builtin_command("compact").map(|item| item.name),
            Some("compact")
        );
        assert!(find_builtin_command("missing").is_none());
    }
}

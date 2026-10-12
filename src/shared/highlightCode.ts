/** 代码块里一个上色片段。plain 保持正文色。 */
export type CodeTokenKind =
  | "plain"
  | "comment"
  | "keyword"
  | "string"
  | "number"
  | "function"
  | "type"
  | "property"
  | "constant";

/** 高亮器输出的最小片段，按源码顺序拼接后必须等于原文。 */
export interface CodeToken {
  kind: CodeTokenKind;
  text: string;
}

/** 某一类语言要打开的着色规则。 */
interface HighlightFamily {
  keywords: ReadonlySet<string>;
  constants: ReadonlySet<string>;
  builtins: ReadonlySet<string>;
  lineComments: readonly string[];
  blockComment?: readonly [string, string];
  hashComment: boolean;
  tripleQuote: boolean;
  template: boolean;
  jsonKeys: boolean;
  bareKeys: boolean;
  callNames: boolean;
  capitalTypes: boolean;
  macroBang: boolean;
  cssIdents: boolean;
  hexColor: boolean;
  atRules: boolean;
  markupTags: boolean;
}

const EMPTY_SET: ReadonlySet<string> = new Set();

/** 把空格分隔的单词收成查找表。 */
function wordSet(words: string): ReadonlySet<string> {
  return new Set(words.split(/\s+/).filter(Boolean));
}

const JS_KEYWORDS = wordSet(`
  abstract as async await break case catch class const continue debugger default delete
  do else enum export extends finally for from function if implements import in
  instanceof interface let namespace new of package private protected public readonly
  return static super switch this throw try typeof var void while yield declare type satisfies
`);

const JS_CONSTANTS = wordSet("true false null undefined NaN Infinity");
const JS_BUILTINS = wordSet("string number boolean any unknown never object symbol bigint");

const RUST_KEYWORDS = wordSet(`
  as async await break const continue crate dyn else enum extern fn for if impl in
  let loop match mod move mut pub ref return self Self static struct super trait
  type unsafe use where while
`);
const RUST_BUILTINS = wordSet("i8 i16 i32 i64 i128 isize u8 u16 u32 u64 u128 usize f32 f64 bool char str");

const PYTHON_KEYWORDS = wordSet(`
  and as assert async await break class continue def del elif else except finally
  for from global if import in is lambda nonlocal not or pass raise return try
  while with yield
`);
const PYTHON_CONSTANTS = wordSet("True False None");

const GO_KEYWORDS = wordSet(`
  break case chan const continue default defer else fallthrough for func go goto
  if import interface map package range return select struct switch type var
`);
const GO_CONSTANTS = wordSet("true false nil iota");
const GO_BUILTINS = wordSet("string int int32 int64 uint uint64 float32 float64 bool byte rune error");

const SHELL_KEYWORDS = wordSet("if then else elif fi for while do done case esac in function return select until");

const SQL_KEYWORDS = wordSet(`
  select from where and or not insert into values update set delete create table
  drop alter join left right inner outer on as group by order limit offset having
  distinct union all is null like in between exists primary key references index
`);

const C_KEYWORDS = wordSet(`
  auto break case char const continue default do else enum extern for goto if
  inline long register return short signed sizeof static struct switch typedef
  union unsigned void volatile while class public private protected namespace
  template typename using virtual override new delete this try catch throw
  bool int float double void
`);

const NONE_FAMILY: HighlightFamily = {
  keywords: EMPTY_SET,
  constants: EMPTY_SET,
  builtins: EMPTY_SET,
  lineComments: [],
  hashComment: false,
  tripleQuote: false,
  template: false,
  jsonKeys: false,
  bareKeys: false,
  callNames: false,
  capitalTypes: false,
  macroBang: false,
  cssIdents: false,
  hexColor: false,
  atRules: false,
  markupTags: false,
};

const GENERIC_FAMILY: HighlightFamily = {
  ...NONE_FAMILY,
  lineComments: ["//"],
  blockComment: ["/*", "*/"],
};

const JS_FAMILY: HighlightFamily = {
  ...GENERIC_FAMILY,
  keywords: JS_KEYWORDS,
  constants: JS_CONSTANTS,
  builtins: JS_BUILTINS,
  template: true,
  callNames: true,
  capitalTypes: true,
};

const JSON_FAMILY: HighlightFamily = {
  ...GENERIC_FAMILY,
  constants: wordSet("true false null"),
  jsonKeys: true,
};

const RUST_FAMILY: HighlightFamily = {
  ...GENERIC_FAMILY,
  keywords: RUST_KEYWORDS,
  constants: wordSet("true false"),
  builtins: RUST_BUILTINS,
  callNames: true,
  capitalTypes: true,
  macroBang: true,
};

const PYTHON_FAMILY: HighlightFamily = {
  ...NONE_FAMILY,
  keywords: PYTHON_KEYWORDS,
  constants: PYTHON_CONSTANTS,
  builtins: wordSet("str int float bool list dict set tuple bytes"),
  hashComment: true,
  tripleQuote: true,
  callNames: true,
  capitalTypes: true,
};

const SHELL_FAMILY: HighlightFamily = {
  ...NONE_FAMILY,
  keywords: SHELL_KEYWORDS,
  hashComment: true,
  callNames: true,
};

const SQL_FAMILY: HighlightFamily = {
  ...NONE_FAMILY,
  keywords: SQL_KEYWORDS,
  constants: wordSet("true false null"),
  lineComments: ["--"],
  blockComment: ["/*", "*/"],
};

const C_FAMILY: HighlightFamily = {
  ...GENERIC_FAMILY,
  keywords: C_KEYWORDS,
  constants: wordSet("true false NULL nullptr"),
  callNames: true,
  capitalTypes: true,
};

const GO_FAMILY: HighlightFamily = {
  ...GENERIC_FAMILY,
  keywords: GO_KEYWORDS,
  constants: GO_CONSTANTS,
  builtins: GO_BUILTINS,
  callNames: true,
  capitalTypes: true,
};

const CSS_FAMILY: HighlightFamily = {
  ...NONE_FAMILY,
  blockComment: ["/*", "*/"],
  bareKeys: true,
  cssIdents: true,
  hexColor: true,
  atRules: true,
};

const MARKUP_FAMILY: HighlightFamily = {
  ...NONE_FAMILY,
  blockComment: ["<!--", "-->"],
  markupTags: true,
};

const DATA_FAMILY: HighlightFamily = {
  ...NONE_FAMILY,
  constants: wordSet("true false null yes no"),
  hashComment: true,
  bareKeys: true,
};

const LANGUAGE_FAMILY: Record<string, HighlightFamily> = {
  js: JS_FAMILY,
  javascript: JS_FAMILY,
  jsx: JS_FAMILY,
  mjs: JS_FAMILY,
  cjs: JS_FAMILY,
  ts: JS_FAMILY,
  typescript: JS_FAMILY,
  tsx: JS_FAMILY,
  mts: JS_FAMILY,
  cts: JS_FAMILY,
  json: JSON_FAMILY,
  jsonc: JSON_FAMILY,
  json5: JSON_FAMILY,
  rs: RUST_FAMILY,
  rust: RUST_FAMILY,
  py: PYTHON_FAMILY,
  python: PYTHON_FAMILY,
  pyi: PYTHON_FAMILY,
  sh: SHELL_FAMILY,
  bash: SHELL_FAMILY,
  zsh: SHELL_FAMILY,
  shell: SHELL_FAMILY,
  fish: SHELL_FAMILY,
  sql: SQL_FAMILY,
  css: CSS_FAMILY,
  scss: CSS_FAMILY,
  less: CSS_FAMILY,
  html: MARKUP_FAMILY,
  xml: MARKUP_FAMILY,
  svg: MARKUP_FAMILY,
  go: GO_FAMILY,
  golang: GO_FAMILY,
  c: C_FAMILY,
  h: C_FAMILY,
  cpp: C_FAMILY,
  cc: C_FAMILY,
  cxx: C_FAMILY,
  hpp: C_FAMILY,
  java: C_FAMILY,
  kt: C_FAMILY,
  kotlin: C_FAMILY,
  cs: C_FAMILY,
  csharp: C_FAMILY,
  swift: C_FAMILY,
  php: JS_FAMILY,
  yaml: DATA_FAMILY,
  yml: DATA_FAMILY,
  toml: DATA_FAMILY,
  rb: DATA_FAMILY,
  ruby: DATA_FAMILY,
  text: NONE_FAMILY,
  txt: NONE_FAMILY,
  plain: NONE_FAMILY,
  plaintext: NONE_FAMILY,
  md: NONE_FAMILY,
  markdown: NONE_FAMILY,
};

const LANGUAGE_LABELS: Record<string, string> = {
  js: "JavaScript",
  javascript: "JavaScript",
  jsx: "JSX",
  ts: "TypeScript",
  typescript: "TypeScript",
  tsx: "TSX",
  json: "JSON",
  jsonc: "JSON",
  rs: "Rust",
  rust: "Rust",
  py: "Python",
  python: "Python",
  sh: "Shell",
  bash: "Bash",
  zsh: "Shell",
  shell: "Shell",
  css: "CSS",
  scss: "SCSS",
  html: "HTML",
  xml: "XML",
  go: "Go",
  golang: "Go",
  sql: "SQL",
  yaml: "YAML",
  yml: "YAML",
  toml: "TOML",
  md: "Markdown",
  markdown: "Markdown",
  c: "C",
  cpp: "C++",
  java: "Java",
  kt: "Kotlin",
  kotlin: "Kotlin",
  cs: "C#",
  csharp: "C#",
  swift: "Swift",
  php: "PHP",
  rb: "Ruby",
  ruby: "Ruby",
  text: "Text",
  plaintext: "Text",
};

/** 把围栏语言名显示成常见写法；不认识的名字保持原文。 */
export function codeBlockLanguageLabel(language: string): string {
  const normalized = language.trim().toLowerCase();
  return LANGUAGE_LABELS[normalized] ?? language.trim();
}

/** 按语言把源码切成 token。未知语言仍给注释、字符串和数字上色。 */
export function highlightCode(source: string, language?: string): CodeToken[] {
  if (!source) {
    return [];
  }

  const family = resolveFamily(language);
  if (family === NONE_FAMILY) {
    return [{ kind: "plain", text: source }];
  }

  const tokens: CodeToken[] = [];
  const length = source.length;
  let index = 0;

  while (index < length) {
    const next = scanToken(source, index, family);
    // 扫描器必须前进，否则坏输入会卡死渲染。
    const end = next.end > index ? next.end : index + 1;
    pushToken(tokens, next.kind, source.slice(index, end));
    index = end;
  }

  return tokens;
}

/** 语言名先取围栏信息，缺省时用通用规则。 */
function resolveFamily(language: string | undefined): HighlightFamily {
  const normalized = language?.trim().toLowerCase() ?? "";
  if (!normalized) {
    return GENERIC_FAMILY;
  }

  return LANGUAGE_FAMILY[normalized] ?? GENERIC_FAMILY;
}

/** 相邻同类 token 合并，减少代码块里的 span 数量。 */
function pushToken(tokens: CodeToken[], kind: CodeTokenKind, text: string) {
  if (!text) {
    return;
  }

  const last = tokens.at(-1);
  if (last && last.kind === kind) {
    last.text += text;
    return;
  }

  tokens.push({ kind, text });
}

/** 从当前位置切出一个 token。 */
function scanToken(source: string, index: number, family: HighlightFamily): { kind: CodeTokenKind; end: number } {
  const lineComment = family.lineComments.find((marker) => source.startsWith(marker, index));
  if (lineComment) {
    return { kind: "comment", end: lineEnd(source, index) };
  }

  if (family.blockComment && source.startsWith(family.blockComment[0], index)) {
    const closeAt = source.indexOf(family.blockComment[1], index + family.blockComment[0].length);
    const end = closeAt === -1 ? source.length : closeAt + family.blockComment[1].length;
    return { kind: "comment", end };
  }

  // $#、${#...} 里的 # 是 shell 变量，不是注释。
  if (family.hashComment && isHashComment(source, index)) {
    return { kind: "comment", end: lineEnd(source, index) };
  }

  if (family.hexColor && source[index] === "#") {
    const hexLength = matchHexColor(source, index);
    if (hexLength > 0) {
      return { kind: "number", end: index + hexLength };
    }
  }

  if (family.tripleQuote && (source.startsWith('"""', index) || source.startsWith("'''", index))) {
    return { kind: "string", end: scanTripleQuote(source, index) };
  }

  const quote = source[index];
  if (quote === '"' || quote === "'" || (quote === "`" && family.template)) {
    const end = scanQuoted(source, index, quote, quote === "`");
    return { kind: classifyString(source, index, end, quote, family), end };
  }

  if (isDigit(source[index])) {
    return { kind: "number", end: scanNumber(source, index) };
  }

  if (family.atRules && quote === "@") {
    const end = scanIdent(source, index + 1, family);
    if (end > index + 1) {
      return { kind: "keyword", end };
    }
  }

  if (family.markupTags && quote === "<") {
    const end = scanMarkupTag(source, index);
    if (end > index) {
      return { kind: "keyword", end };
    }
  }

  if (isIdentStart(quote)) {
    const end = scanIdent(source, index, family);
    return { kind: classifyWord(source.slice(index, end), source, end, family), end };
  }

  let end = index + 1;
  while (end < source.length && !isSpecialStart(source, end, family)) {
    end += 1;
  }

  return { kind: "plain", end };
}

/** 行注释吃到换行，换行符留给下一行重新判断。 */
function lineEnd(source: string, index: number): number {
  const newline = source.indexOf("\n", index);
  return newline === -1 ? source.length : newline;
}

/** # 只有在一行的起始位置，或跟在空白和分隔符后面，才是注释。 */
function isHashComment(source: string, index: number): boolean {
  if (source[index] !== "#") {
    return false;
  }

  if (index === 0) {
    return true;
  }

  const previous = source[index - 1];
  return previous === " " || previous === "\t" || previous === "\n" || previous === "\r" || previous === ";" || previous === "(" || previous === "{";
}

/** CSS 颜色 #rgb / #rrggbb / #rrggbbaa。后面还是十六进制就不算完整颜色。 */
function matchHexColor(source: string, index: number): number {
  let end = index + 1;
  while (end < source.length && end - index <= 9 && isHex(source[end])) {
    end += 1;
  }

  const length = end - index;
  if (length !== 4 && length !== 7 && length !== 9) {
    return 0;
  }

  return length;
}

function scanTripleQuote(source: string, index: number): number {
  const marker = source.slice(index, index + 3);
  const closeAt = source.indexOf(marker, index + 3);
  return closeAt === -1 ? source.length : closeAt + 3;
}

/** 扫描引号字符串。未闭合时停在换行前，避免把后面的代码整段染成字符串。 */
function scanQuoted(source: string, index: number, quote: string, multiline: boolean): number {
  let end = index + 1;

  while (end < source.length) {
    const ch = source[end];
    if (ch === "\\") {
      end += end + 1 < source.length ? 2 : 1;
      continue;
    }

    if (!multiline && (ch === "\n" || ch === "\r")) {
      break;
    }

    end += 1;
    if (ch === quote) {
      return end;
    }
  }

  return end;
}

/** JSON 里冒号前的字符串是键，其余引号内容是普通字符串。 */
function classifyString(
  source: string,
  index: number,
  end: number,
  quote: string,
  family: HighlightFamily,
): CodeTokenKind {
  if (!family.jsonKeys || quote !== '"' || source[end - 1] !== '"') {
    return "string";
  }

  let cursor = end;
  while (cursor < source.length && (source[cursor] === " " || source[cursor] === "\t")) {
    cursor += 1;
  }

  return source[cursor] === ":" ? "property" : "string";
}

function scanNumber(source: string, index: number): number {
  let end = index;
  if ((source[end] === "0" && (source[end + 1] === "x" || source[end + 1] === "X")) || false) {
    end += 2;
    while (end < source.length && (isHex(source[end]) || source[end] === "_")) {
      end += 1;
    }
    return end;
  }

  while (end < source.length && (isDigit(source[end]) || source[end] === "_")) {
    end += 1;
  }

  if (source[end] === "." && isDigit(source[end + 1] ?? "")) {
    end += 1;
    while (end < source.length && (isDigit(source[end]) || source[end] === "_")) {
      end += 1;
    }
  }

  if (source[end] === "e" || source[end] === "E") {
    const sign = source[end + 1];
    const digitAt = sign === "+" || sign === "-" ? end + 2 : end + 1;
    if (isDigit(source[digitAt] ?? "")) {
      end = digitAt;
      while (end < source.length && (isDigit(source[end]) || source[end] === "_")) {
        end += 1;
      }
    }
  }

  return end;
}

function scanIdent(source: string, index: number, family: HighlightFamily): number {
  let end = index;
  if (isIdentStart(source[end])) {
    end += 1;
  }

  while (end < source.length && isIdentPart(source[end], family)) {
    end += 1;
  }

  return end;
}

/** <div 或 </div 染成标签色，> 留给后面的普通文本。 */
function scanMarkupTag(source: string, index: number): number {
  let end = index + 1;
  if (source[end] === "/") {
    end += 1;
  }

  const nameStart = end;
  while (end < source.length && /[A-Za-z0-9:-]/.test(source[end])) {
    end += 1;
  }

  return end > nameStart ? end : index;
}

/** 标识符按关键字、常量、函数调用和类型名归类。 */
function classifyWord(word: string, source: string, end: number, family: HighlightFamily): CodeTokenKind {
  if (family.keywords.has(word)) {
    return "keyword";
  }

  if (family.constants.has(word)) {
    return "constant";
  }

  if (family.builtins.has(word)) {
    return "type";
  }

  const next = nextNonSpace(source, end);
  if (family.callNames && source[next] === "(") {
    return "function";
  }

  // Rust 宏 println! 和不等号 != 分开。
  if (family.macroBang && source[end] === "!" && source[end + 1] !== "=") {
    return "function";
  }

  if (family.bareKeys && isBareKey(source, next)) {
    return "property";
  }

  if (family.capitalTypes && /^[A-Z]/.test(word)) {
    return "type";
  }

  return "plain";
}

/** YAML / CSS 里，标识符紧跟冒号时是键名。双冒号留给 Rust 这类路径。 */
function isBareKey(source: string, index: number): boolean {
  return source[index] === ":" && source[index + 1] !== ":";
}

function nextNonSpace(source: string, index: number): number {
  let cursor = index;
  while (cursor < source.length && (source[cursor] === " " || source[cursor] === "\t")) {
    cursor += 1;
  }
  return cursor;
}

/** 下一个字符会开启注释、字符串、数字或标识符时，普通文本要停下来。 */
function isSpecialStart(source: string, index: number, family: HighlightFamily): boolean {
  const ch = source[index];
  if (ch === '"' || ch === "'") {
    return true;
  }

  if (ch === "`" && family.template) {
    return true;
  }

  if (isIdentStart(ch) || isDigit(ch)) {
    return true;
  }

  if (family.lineComments.some((marker) => source.startsWith(marker, index))) {
    return true;
  }

  if (family.blockComment && source.startsWith(family.blockComment[0], index)) {
    return true;
  }

  if (family.hashComment && isHashComment(source, index)) {
    return true;
  }

  if (family.hexColor && ch === "#") {
    return true;
  }

  if (family.tripleQuote && (source.startsWith('"""', index) || source.startsWith("'''", index))) {
    return true;
  }

  if (family.atRules && ch === "@") {
    return true;
  }

  if (family.markupTags && ch === "<") {
    return true;
  }

  return false;
}

function isIdentStart(ch: string | undefined): boolean {
  return Boolean(ch && /[A-Za-z_$]/.test(ch));
}

function isIdentPart(ch: string, family: HighlightFamily): boolean {
  if (/[A-Za-z0-9_$]/.test(ch)) {
    return true;
  }

  return family.cssIdents && ch === "-";
}

function isDigit(ch: string | undefined): boolean {
  return Boolean(ch && ch >= "0" && ch <= "9");
}

function isHex(ch: string | undefined): boolean {
  return Boolean(ch && /[0-9a-fA-F]/.test(ch));
}

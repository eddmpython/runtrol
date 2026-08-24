/// Colour for the code an agent writes back, in the same shape the rest of this webview speaks.
///
/// DOM-free, exactly like `markdown.ts` and for the same reason: this returns descriptors and the page
/// materialises them with `createElement`/`createTextNode`, so no line of a conversation ever becomes HTML and
/// `innerHTML` stays banned. That is also why no highlighting library is used. The maintained ones emit HTML
/// strings, and rendering one would hand untrusted model output straight to the parser.
///
/// The grammar is deliberately shallow: comments, strings, numbers, keywords, types and called names. That is
/// the part of a language a reader uses to find their place in a block of code, and it is the part a scanner can
/// get right without a parser. Anything it cannot classify stays plain text, which is exactly what the code
/// block showed before this module existed, so a wrong guess costs a colour and never a character.

export type TokenKind =
  | "plain"
  | "comment"
  | "string"
  | "number"
  | "keyword"
  | "type"
  | "function";

export type Token = {
  readonly kind: TokenKind;
  readonly text: string;
};

type Grammar = {
  /// Where a comment runs to the end of the line, longest marker first.
  readonly lineComment: readonly string[];
  /// Paired block comment markers, or none.
  readonly blockComment: readonly (readonly [string, string])[];
  /// Quote characters that open a string.
  readonly quotes: readonly string[];
  /// Whether a backslash escapes the next character inside a string.
  readonly escapes: boolean;
  readonly keywords: ReadonlySet<string>;
  readonly types: ReadonlySet<string>;
};

function words(list: string): ReadonlySet<string> {
  return new Set(list.split(" "));
}

const SCRIPT: Grammar = {
  lineComment: ["//"],
  blockComment: [["/*", "*/"]],
  quotes: ["\"", "'", "`"],
  escapes: true,
  keywords: words(
    "as async await break case catch class const continue debugger default delete do else enum export extends"
    + " finally for from function get if implements import in instanceof interface let new of readonly return"
    + " satisfies set static super switch this throw try type typeof var void while yield declare namespace",
  ),
  types: words(
    "true false null undefined NaN Infinity string number boolean object symbol bigint any unknown never void"
    + " Array Promise Map Set Record Object String Number Boolean Error Date JSON Math console",
  ),
};

const PYTHON: Grammar = {
  lineComment: ["#"],
  blockComment: [],
  quotes: ["\"", "'"],
  escapes: true,
  keywords: words(
    "and as assert async await break class continue def del elif else except finally for from global if import"
    + " in is lambda nonlocal not or pass raise return try while with yield match case",
  ),
  types: words(
    "True False None self cls int float str bool bytes list dict set tuple object type len range print"
    + " Exception ValueError TypeError KeyError IndexError",
  ),
};

const RUST: Grammar = {
  lineComment: ["//"],
  blockComment: [["/*", "*/"]],
  quotes: ["\"", "'"],
  escapes: true,
  keywords: words(
    "as async await break const continue crate dyn else enum extern fn for if impl in let loop match mod move"
    + " mut pub ref return self Self static struct super trait type unsafe use where while",
  ),
  types: words(
    "true false None Some Ok Err bool char str String u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize f32"
    + " f64 Vec Option Result Box Rc Arc HashMap HashSet BTreeMap",
  ),
};

const SHELL: Grammar = {
  lineComment: ["#"],
  blockComment: [],
  quotes: ["\"", "'"],
  escapes: true,
  keywords: words(
    "if then elif else fi for while until do done case esac function in return export local readonly set unset"
    + " source echo cd exit shift trap",
  ),
  types: words("true false"),
};

const JSON_LIKE: Grammar = {
  lineComment: [],
  blockComment: [],
  quotes: ["\""],
  escapes: true,
  keywords: new Set<string>(),
  types: words("true false null"),
};

/// Only strings, comments and numbers. Enough to find your place in a language this module has no words for,
/// without claiming to know which of its identifiers are keywords.
const PLAIN: Grammar = {
  lineComment: ["#", "//"],
  blockComment: [["/*", "*/"]],
  quotes: ["\"", "'", "`"],
  escapes: true,
  keywords: new Set<string>(),
  types: new Set<string>(),
};

/// The names agents actually write after a fence, mapped to the grammar that reads them.
const GRAMMARS = new Map<string, Grammar>([
  ["ts", SCRIPT], ["tsx", SCRIPT], ["typescript", SCRIPT],
  ["js", SCRIPT], ["jsx", SCRIPT], ["javascript", SCRIPT], ["mjs", SCRIPT], ["cjs", SCRIPT],
  ["py", PYTHON], ["python", PYTHON],
  ["rs", RUST], ["rust", RUST],
  ["sh", SHELL], ["bash", SHELL], ["zsh", SHELL], ["shell", SHELL], ["console", SHELL],
  ["json", JSON_LIKE], ["jsonc", JSON_LIKE],
]);

/// Whether this fence language gets more than the shared string-and-comment reading.
export function isKnownLanguage(language: string): boolean {
  return GRAMMARS.has(language.trim().toLowerCase());
}

/// One code block as coloured runs, in source order. Concatenating every `text` returns the input exactly.
export function highlight(language: string, text: string): Token[] {
  const grammar = GRAMMARS.get(language.trim().toLowerCase()) ?? PLAIN;
  const tokens: Token[] = [];
  let plain = "";
  const flush = (): void => {
    if (plain) {
      tokens.push({ kind: "plain", text: plain });
      plain = "";
    }
  };
  const take = (kind: TokenKind, value: string): void => {
    flush();
    tokens.push({ kind, text: value });
  };

  let index = 0;
  while (index < text.length) {
    const rest = text.slice(index);

    const line = grammar.lineComment.find((marker) => rest.startsWith(marker));
    if (line) {
      const end = text.indexOf("\n", index);
      const stop = end === -1 ? text.length : end;
      take("comment", text.slice(index, stop));
      index = stop;
      continue;
    }

    const block = grammar.blockComment.find(([open]) => rest.startsWith(open));
    if (block) {
      const closeAt = text.indexOf(block[1], index + block[0].length);
      // A fence that is still streaming can end inside a comment. Colouring to the end is what the reader sees
      // in an editor too, and the closing marker recolours nothing when it arrives because the block reparses.
      const stop = closeAt === -1 ? text.length : closeAt + block[1].length;
      take("comment", text.slice(index, stop));
      index = stop;
      continue;
    }

    const quote = grammar.quotes.find((mark) => rest.startsWith(mark));
    if (quote) {
      let cursor = index + quote.length;
      while (cursor < text.length) {
        if (grammar.escapes && text[cursor] === "\\") {
          cursor += 2;
          continue;
        }
        if (text.startsWith(quote, cursor)) {
          cursor += quote.length;
          break;
        }
        // A single-quoted or double-quoted string does not survive a line break in these languages, so an
        // unterminated one stops at the newline rather than colouring the rest of the block.
        if (text[cursor] === "\n" && quote !== "`") break;
        cursor += 1;
      }
      take("string", text.slice(index, Math.min(cursor, text.length)));
      index = Math.min(cursor, text.length);
      continue;
    }

    const character = text[index] ?? "";
    if (isDigit(character) && !isWordCharacter(text[index - 1] ?? "")) {
      let cursor = index;
      while (cursor < text.length && isNumberCharacter(text[cursor] ?? "")) cursor += 1;
      take("number", text.slice(index, cursor));
      index = cursor;
      continue;
    }

    if (isWordStart(character)) {
      let cursor = index;
      while (cursor < text.length && isWordCharacter(text[cursor] ?? "")) cursor += 1;
      const word = text.slice(index, cursor);
      if (grammar.keywords.has(word)) take("keyword", word);
      else if (grammar.types.has(word)) take("type", word);
      else if (isCall(text, cursor)) take("function", word);
      else plain += word;
      index = cursor;
      continue;
    }

    plain += character;
    index += 1;
  }
  flush();
  return tokens;
}

/// A name immediately followed by an opening bracket is being called, which is the one structural fact worth a
/// colour without a parser.
function isCall(text: string, from: number): boolean {
  let cursor = from;
  while (cursor < text.length && (text[cursor] === " " || text[cursor] === "\t")) cursor += 1;
  return text[cursor] === "(";
}

function isDigit(value: string): boolean {
  return value >= "0" && value <= "9";
}

function isNumberCharacter(value: string): boolean {
  return isDigit(value) || value === "." || value === "_"
    || (value >= "a" && value <= "f") || (value >= "A" && value <= "F")
    || value === "x" || value === "X" || value === "o" || value === "b";
}

function isWordStart(value: string): boolean {
  return (value >= "a" && value <= "z") || (value >= "A" && value <= "Z") || value === "_" || value === "$";
}

function isWordCharacter(value: string): boolean {
  return isWordStart(value) || isDigit(value);
}

use crate::sql_text;

// ---------------------------------------------------------------------------
// 1) LexMode – replaces 6 boolean flags with a single enum.
//    Illegal states (e.g. in_single_quote && in_block_comment) are now
//    structurally impossible.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) enum LexMode {
    /// Normal code – not inside any string literal or comment.
    #[default]
    Idle,
    SingleQuote,
    DoubleQuote,
    BacktickQuote,
    LineComment,
    BlockComment,
    QQuote {
        end_char: char,
        depth: usize,
    },
    DollarQuote {
        tag: String,
    },
}

// ---------------------------------------------------------------------------
// 2) BlockKind stack – replaces (block_depth: usize, case_depth_stack: Vec).
//    Each entry records *what* opened the block so END resolution is
//    unambiguous – no more guessing based on depth arithmetic.
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum BlockKind {
    /// BEGIN ... END  (standalone or DECLARE ... BEGIN ... END)
    Begin,
    /// DECLARE ... (waiting for BEGIN, shares depth with subsequent BEGIN)
    Declare,
    /// AS/IS ... BEGIN ... END  (CREATE PL/SQL body)
    AsIs,
    /// CASE ... END [CASE]  (could be expression or statement)
    Case,
    /// IF ... THEN ... END IF
    If,
    /// LOOP ... END LOOP  /  FOR ... LOOP ... END LOOP
    Loop,
    /// WHILE ... DO ... END WHILE  (MySQL-style)
    While,
    /// FOR ... DO ... END FOR (MySQL-style)
    For,
    /// REPEAT ... END REPEAT
    Repeat,
    /// COMPOUND (TRIGGER body outer block)
    Compound,
    /// BEFORE/AFTER/INSTEAD timing point IS ... END <timing>
    TimingPoint,
}

// ---------------------------------------------------------------------------
// 3) Pending state machines – replaces pending_end, pending_if_*,
//    pending_while_do/pending_for_do, pending_timing_point_is.
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub(crate) enum PendingEnd {
    #[default]
    None,
    /// Saw END, waiting for next token to determine what it closes.
    End,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PendingEndSuffix {
    Case,
    If,
    Loop,
    While,
    Repeat,
    For,
    TimingPoint,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum EndTokenRole {
    None,
    Suffix(PendingEndSuffix),
}

impl EndTokenRole {
    fn from_token(
        token_upper: &str,
        pending_end: PendingEnd,
        allow_timing_point_suffix: bool,
    ) -> Self {
        if pending_end != PendingEnd::End {
            return Self::None;
        }

        PendingEndSuffix::parse(token_upper, allow_timing_point_suffix)
            .map(Self::Suffix)
            .unwrap_or(Self::None)
    }

    fn suffix(self) -> Option<PendingEndSuffix> {
        match self {
            Self::Suffix(suffix) => Some(suffix),
            Self::None => None,
        }
    }

    fn is_suffix(self, suffix: PendingEndSuffix) -> bool {
        self.suffix() == Some(suffix)
    }
}

impl PendingEndSuffix {
    fn parse(token_upper: &str, allow_timing_point_suffix: bool) -> Option<Self> {
        match token_upper {
            "CASE" => Some(Self::Case),
            "IF" => Some(Self::If),
            "LOOP" => Some(Self::Loop),
            "WHILE" => Some(Self::While),
            "REPEAT" => Some(Self::Repeat),
            "FOR" => Some(Self::For),
            "BEFORE" | "AFTER" | "INSTEAD" if allow_timing_point_suffix => Some(Self::TimingPoint),
            _ => None,
        }
    }

    fn apply_to_state(self, state: &mut SplitState) -> bool {
        let popped = match self {
            Self::Case => {
                state.pop_case_block()
            }
            Self::If => {
                state.pop_top_matching_block(&[BlockKind::If])
            }
            Self::Loop => {
                state.pop_top_matching_block(&[BlockKind::Loop])
            }
            Self::While => {
                state.pop_top_matching_block(&[BlockKind::While, BlockKind::Loop])
            }
            Self::Repeat => {
                state.pop_top_matching_block(&[BlockKind::Repeat])
            }
            Self::For => {
                state.pop_top_matching_block(&[BlockKind::For, BlockKind::Loop])
            }
            Self::TimingPoint => state.pop_timing_point_block(),
        };

        if self == Self::TimingPoint {
            state.timing_point_state = TimingPointState::None;
        }

        popped
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) enum IfState {
    #[default]
    None,
    /// Saw IF, waiting for the first meaningful character after IF.
    ExpectConditionStart,
    /// Saw IF followed by `(`, tracking condition paren depth.
    InConditionParen { depth: usize },
    /// Saw IF (no paren), waiting for THEN.
    AwaitingThen,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) enum PendingDo {
    #[default]
    None,
    While {
        armed_at_block_depth: usize,
    },
    For {
        armed_at_block_depth: usize,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct RoutineFrame {
    block_depth: usize,
    semicolon_policy: SemicolonPolicy,
    external_clause_state: ExternalClauseState,
    implicit_language_target_is_quoted: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SemicolonPolicy {
    Default,
    ForceSplit,
    CloseRoutineBlock,
    AwaitingImplicitTopLevelDecision,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ExternalClauseState {
    None,
    SawExternalKeyword,
    SawUsingClauseSubject,
    SawMleKeyword,
    AwaitingLanguageTargetFromExternal,
    AwaitingLanguageTargetImplicit,
    SawImplicitQuotedLanguageTarget,
    SawImplicitLanguageTarget,
    Confirmed,
}

impl RoutineFrame {
    fn new(block_depth: usize) -> Self {
        Self {
            block_depth,
            semicolon_policy: SemicolonPolicy::Default,
            external_clause_state: ExternalClauseState::None,
            implicit_language_target_is_quoted: false,
        }
    }

    fn should_split_on_semicolon(self, current_block_depth: usize) -> bool {
        self.block_depth == current_block_depth
            && self.semicolon_policy == SemicolonPolicy::ForceSplit
    }

    fn should_close_routine_block_on_semicolon(self, current_block_depth: usize) -> bool {
        self.block_depth == current_block_depth
            && self.semicolon_policy == SemicolonPolicy::CloseRoutineBlock
    }

    fn has_pending_external_clause(self) -> bool {
        matches!(
            self.external_clause_state,
            ExternalClauseState::SawExternalKeyword
                | ExternalClauseState::SawUsingClauseSubject
                | ExternalClauseState::SawMleKeyword
                | ExternalClauseState::AwaitingLanguageTargetFromExternal
                | ExternalClauseState::AwaitingLanguageTargetImplicit
                | ExternalClauseState::SawImplicitQuotedLanguageTarget
                | ExternalClauseState::SawImplicitLanguageTarget
        )
    }

    fn pending_external_clause_requires_top_level_split(self) -> bool {
        matches!(
            self.external_clause_state,
            ExternalClauseState::SawImplicitQuotedLanguageTarget
                | ExternalClauseState::SawImplicitLanguageTarget
                | ExternalClauseState::AwaitingLanguageTargetImplicit
        )
    }

    fn should_defer_begin_split_after_implicit_semicolon(self) -> bool {
        self.implicit_language_target_is_quoted
    }

    fn mark_external_clause(&mut self) {
        self.semicolon_policy = if self.block_depth == 1 {
            SemicolonPolicy::ForceSplit
        } else {
            SemicolonPolicy::CloseRoutineBlock
        };
        self.external_clause_state = ExternalClauseState::Confirmed;
        self.implicit_language_target_is_quoted = false;
    }

    fn mark_implicit_language_target_on_semicolon(&mut self) {
        self.semicolon_policy = if self.block_depth == 1 {
            SemicolonPolicy::AwaitingImplicitTopLevelDecision
        } else {
            SemicolonPolicy::CloseRoutineBlock
        };
        self.external_clause_state = ExternalClauseState::Confirmed;
    }

    fn observe_external_clause_token(&mut self, token_upper: &str, allow_implicit_language: bool) {
        if matches!(
            self.external_clause_state,
            ExternalClauseState::AwaitingLanguageTargetFromExternal
                | ExternalClauseState::AwaitingLanguageTargetImplicit
        ) {
            let from_external = self.external_clause_state
                == ExternalClauseState::AwaitingLanguageTargetFromExternal;
            self.external_clause_state = ExternalClauseState::None;
            if is_external_language_target(token_upper) {
                if from_external {
                    self.mark_external_clause();
                } else {
                    self.external_clause_state = ExternalClauseState::SawImplicitLanguageTarget;
                    self.implicit_language_target_is_quoted = false;
                }
                return;
            }

            if from_external && token_upper.bytes().all(sql_text::is_identifier_byte) {
                self.mark_external_clause();
                return;
            }

            if sql_text::is_external_language_clause_keyword(token_upper) {
                if from_external {
                    // Be permissive for malformed call specs such as
                    // `EXTERNAL LANGUAGE PARAMETERS ...` without an explicit
                    // language target. Once `EXTERNAL` was observed, subsequent
                    // call-spec tokens still belong to an external routine clause
                    // and semicolon handling should keep routine boundaries stable.
                    self.mark_external_clause();
                } else if allow_implicit_language {
                    // Keep malformed implicit call specs (e.g. `AS LANGUAGE PARAMETERS ...`)
                    // in call-spec mode so semicolon handling can still split before the
                    // next top-level statement.
                    self.external_clause_state = ExternalClauseState::SawImplicitLanguageTarget;
                    self.implicit_language_target_is_quoted = false;
                }
                return;
            }
        }

        if token_upper == "EXTERNAL" {
            self.external_clause_state = ExternalClauseState::SawExternalKeyword;
            self.implicit_language_target_is_quoted = false;
            return;
        }

        if matches!(token_upper, "AGGREGATE" | "PIPELINED") {
            self.external_clause_state = ExternalClauseState::SawUsingClauseSubject;
            self.implicit_language_target_is_quoted = false;
            return;
        }

        if token_upper == "MLE" {
            if matches!(
                self.external_clause_state,
                ExternalClauseState::SawImplicitLanguageTarget | ExternalClauseState::Confirmed
            ) {
                self.mark_external_clause();
            } else {
                self.external_clause_state = ExternalClauseState::SawMleKeyword;
                self.implicit_language_target_is_quoted = false;
            }
            return;
        }

        if matches!(
            token_upper,
            "MODULE"
                | "SIGNATURE"
                | "ENV"
                | "ENVIRONMENT"
                | "IMPORT"
                | "IMPORTS"
                | "EXPORT"
                | "EXPORTS"
        ) {
            if matches!(
                self.external_clause_state,
                ExternalClauseState::SawMleKeyword
                    | ExternalClauseState::SawImplicitQuotedLanguageTarget
                    | ExternalClauseState::SawImplicitLanguageTarget
                    | ExternalClauseState::Confirmed
            ) {
                self.mark_external_clause();
            }
            return;
        }

        if token_upper == "USING" {
            if self.external_clause_state == ExternalClauseState::SawUsingClauseSubject {
                self.mark_external_clause();
            }
            return;
        }

        if self.external_clause_state == ExternalClauseState::SawUsingClauseSubject
            && matches!(token_upper, "ROW" | "SCALAR" | "TABLE" | "POLYMORPHIC")
        {
            return;
        }

        if token_upper == "LANGUAGE" {
            if self.external_clause_state == ExternalClauseState::SawExternalKeyword {
                self.external_clause_state =
                    ExternalClauseState::AwaitingLanguageTargetFromExternal;
                self.implicit_language_target_is_quoted = false;
            } else if self.external_clause_state == ExternalClauseState::SawMleKeyword {
                // MLE call specs can include trailing LANGUAGE clauses; keep
                // routine call-spec splitting active when that form appears.
                self.mark_external_clause();
            } else if allow_implicit_language {
                self.external_clause_state = ExternalClauseState::AwaitingLanguageTargetImplicit;
                self.implicit_language_target_is_quoted = false;
            } else {
                self.external_clause_state = ExternalClauseState::None;
                self.implicit_language_target_is_quoted = false;
            }
            return;
        }

        if sql_text::is_external_language_clause_keyword(token_upper) {
            if matches!(
                self.external_clause_state,
                ExternalClauseState::SawExternalKeyword
                    | ExternalClauseState::SawImplicitQuotedLanguageTarget
                    | ExternalClauseState::SawImplicitLanguageTarget
                    | ExternalClauseState::Confirmed
            ) {
                self.mark_external_clause();
            }
            return;
        }

        if matches!(
            self.external_clause_state,
            ExternalClauseState::SawExternalKeyword
                | ExternalClauseState::SawUsingClauseSubject
                | ExternalClauseState::SawMleKeyword
                | ExternalClauseState::SawImplicitQuotedLanguageTarget
        ) {
            self.external_clause_state = ExternalClauseState::None;
            self.implicit_language_target_is_quoted = false;
        }
    }

    fn observe_external_clause_quoted_identifier_target(&mut self) {
        match self.external_clause_state {
            ExternalClauseState::AwaitingLanguageTargetFromExternal => {
                self.mark_external_clause();
            }
            ExternalClauseState::AwaitingLanguageTargetImplicit => {
                self.external_clause_state = ExternalClauseState::SawImplicitQuotedLanguageTarget;
                self.implicit_language_target_is_quoted = true;
            }
            _ => {}
        }
    }

    fn observe_external_clause_literal_target(&mut self, allow_implicit_target: bool) {
        let from_external = match self.external_clause_state {
            ExternalClauseState::AwaitingLanguageTargetFromExternal => true,
            ExternalClauseState::AwaitingLanguageTargetImplicit => false,
            _ => return,
        };

        self.external_clause_state = ExternalClauseState::None;
        if from_external {
            self.mark_external_clause();
        } else if allow_implicit_target {
            self.external_clause_state = ExternalClauseState::SawImplicitLanguageTarget;
            self.implicit_language_target_is_quoted = false;
        }
    }

    fn observe_external_clause_symbol(&mut self, ch: char, next: Option<char>) {
        if !matches!(
            self.external_clause_state,
            ExternalClauseState::AwaitingLanguageTargetFromExternal
                | ExternalClauseState::AwaitingLanguageTargetImplicit
        ) {
            return;
        }

        if ch.is_whitespace() {
            return;
        }

        if self.external_clause_state == ExternalClauseState::AwaitingLanguageTargetImplicit
            && matches!(ch, '"' | '`')
        {
            self.external_clause_state = ExternalClauseState::None;
            return;
        }

        let is_canceling_symbol = matches!(
            ch,
            ':' | '='
                | '+'
                | '*'
                | '%'
                | '<'
                | '>'
                | '|'
                | ','
                | '.'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
        ) || (ch == '-' && next != Some('-'))
            || (ch == '/' && next != Some('*'));

        if is_canceling_symbol {
            self.external_clause_state = ExternalClauseState::None;
            self.implicit_language_target_is_quoted = false;
        }
    }

    fn finalize_external_clause_on_semicolon(&mut self, allow_implicit_target_split: bool) {
        match self.external_clause_state {
            ExternalClauseState::SawExternalKeyword
            | ExternalClauseState::AwaitingLanguageTargetFromExternal
            | ExternalClauseState::SawUsingClauseSubject
            | ExternalClauseState::SawMleKeyword => {
                self.mark_external_clause();
            }
            ExternalClauseState::SawImplicitQuotedLanguageTarget => {
                if allow_implicit_target_split {
                    self.mark_implicit_language_target_on_semicolon();
                    self.implicit_language_target_is_quoted = true;
                } else {
                    self.external_clause_state = ExternalClauseState::None;
                    self.implicit_language_target_is_quoted = false;
                }
            }
            ExternalClauseState::SawImplicitLanguageTarget
            | ExternalClauseState::AwaitingLanguageTargetImplicit => {
                if allow_implicit_target_split {
                    self.mark_implicit_language_target_on_semicolon();
                } else {
                    self.external_clause_state = ExternalClauseState::None;
                }
            }
            _ => {}
        }
    }
}

impl PendingDo {
    fn arm_for_token(self, token_upper: &str, armed_at_block_depth: usize) -> Self {
        if self != Self::None {
            return self;
        }

        match token_upper {
            "WHILE" => Self::While {
                armed_at_block_depth,
            },
            "FOR" => Self::For {
                armed_at_block_depth,
            },
            _ => Self::None,
        }
    }

    fn resolve_do(self, current_block_depth: usize) -> Option<BlockKind> {
        match self {
            Self::While {
                armed_at_block_depth,
            } if armed_at_block_depth == current_block_depth => Some(BlockKind::While),
            Self::For {
                armed_at_block_depth,
            } if armed_at_block_depth == current_block_depth => Some(BlockKind::For),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum WithClauseState {
    #[default]
    None,
    PendingClause,
    InPlsqlDeclaration(WithDeclarationState),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum WithDeclarationState {
    CollectingDeclaration,
    AwaitingMainQuery,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum TopLevelTokenState {
    #[default]
    NoneSeen,
    Seen,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum TimingPointState {
    #[default]
    None,
    AwaitingAsOrIs,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum BeginState {
    #[default]
    None,
    AfterDeclare,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum AsIsState {
    #[default]
    None,
    AwaitingNestedSubprogram,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum AsIsFollowState {
    #[default]
    None,
    AwaitingTypeDeclarativeKind,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum IfSymbolEvent {
    Whitespace,
    OpenParen,
    Dot,
    Other,
}

impl IfSymbolEvent {
    fn from_char(ch: char) -> Self {
        if ch.is_whitespace() {
            return Self::Whitespace;
        }

        if ch == '(' {
            return Self::OpenParen;
        }

        if ch == '.' {
            return Self::Dot;
        }

        Self::Other
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SymbolRole {
    Semicolon,
    PendingEndSeparator,
    OpenParen,
    CloseParen,
    Other,
}

impl SymbolRole {
    fn from_char(ch: char, next: Option<char>) -> Self {
        if ch == ';' {
            return Self::Semicolon;
        }

        if ch == '(' {
            return Self::OpenParen;
        }

        if ch == ')' {
            return Self::CloseParen;
        }

        let is_pending_end_separator = matches!(
            ch,
            ',' | ']' | '}' | '+' | '*' | '%' | '=' | '<' | '>' | '|'
        ) || (ch == '-' && next != Some('-'))
            || (ch == '/' && next != Some('*'));

        if is_pending_end_separator {
            return Self::PendingEndSeparator;
        }

        Self::Other
    }

    fn resolves_pending_end(self) -> bool {
        matches!(self, Self::PendingEndSeparator | Self::CloseParen)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SlashLineKind {
    PureTerminator,
    BlockComment,
    LineComment,
    SqlPlusRemark,
}

impl SlashLineKind {
    pub(crate) fn consumes_as_terminator(self) -> bool {
        matches!(
            self,
            Self::PureTerminator | Self::BlockComment | Self::SqlPlusRemark
        )
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum LineLeadingMarker {
    None,
    RunScriptAt,
    BangHost,
    OpenParen,
    Slash(SlashLineKind),
}

impl LineLeadingMarker {
    pub(crate) fn from_line(line: &str) -> Self {
        let mut trimmed = line.trim_start();

        while let Some(block_body) = trimmed.strip_prefix("/*") {
            let Some(block_end) = block_body.find("*/") else {
                return Self::None;
            };
            trimmed = block_body[block_end + 2..].trim_start();
        }

        match trimmed.chars().next() {
            Some('@') => Self::RunScriptAt,
            Some('!') => Self::BangHost,
            Some('(') => Self::OpenParen,
            Some('/') => classify_line_leading_slash_marker(trimmed)
                .map(Self::Slash)
                .unwrap_or(Self::None),
            _ => Self::None,
        }
    }
}

pub(crate) fn classify_line_leading_marker(line: &str) -> LineLeadingMarker {
    LineLeadingMarker::from_line(line)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum LineBoundaryAction {
    None,
    SplitBeforeLine,
    SplitAndConsumeLine,
    ConsumeLine,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum EndResolutionPolicy {
    KeepCreateState,
    ResetCreateStateWhenTopLevel,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum AsIsBlockStart {
    None,
    Regular,
    TimingPoint,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SemicolonAction {
    AppendToCurrent,
    SplitTopLevel,
    SplitForcedRoutine,
    CloseRoutineBlock,
}

impl SemicolonAction {
    pub(crate) fn from_state(state: &SplitState) -> Self {
        if state.keep_semicolons_inside_create_body() {
            return Self::AppendToCurrent;
        }

        if state.block_depth() == 0 && state.paren_depth() == 0 && !state.in_with_plsql_declaration()
        {
            return Self::SplitTopLevel;
        }

        if state.paren_depth() == 0 && state.should_split_on_semicolon() {
            return Self::SplitForcedRoutine;
        }

        if state.paren_depth() == 0 && state.should_close_routine_block_on_semicolon() {
            return Self::CloseRoutineBlock;
        }

        Self::AppendToCurrent
    }
}

impl AsIsBlockStart {
    fn from_token(upper: &str, state: &SplitState) -> Self {
        if !matches!(upper, "AS" | "IS") {
            return Self::None;
        }

        if state.paren_depth() != 0 {
            return Self::None;
        }

        if state.timing_point_state == TimingPointState::AwaitingAsOrIs {
            return Self::TimingPoint;
        }

        if state.is_trigger()
            && !state.in_compound_trigger()
            && state.block_depth() == 0
            && state.saw_trigger_alias_subject
            && matches!(upper, "AS" | "IS")
        {
            // In simple trigger headers, `REFERENCING NEW/OLD/PARENT AS|IS alias`
            // uses AS/IS for alias clauses, not for opening the declarative section.
            // Ignore only that alias form so `FOR EACH ROW AS ... BEGIN ... END;`
            // still enters the trigger body block correctly.
            return Self::None;
        }

        if state.as_is_state == AsIsState::AwaitingNestedSubprogram
            || (state.collecting_with_plsql_routine_declaration()
                && state.block_depth() == 0)
            || (state.in_create_plsql()
                && !state.in_java_source_create()
                && state.block_depth() == 0)
        {
            return Self::Regular;
        }

        Self::None
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) enum CreateState {
    #[default]
    None,
    AwaitingObjectType,
    AwaitingJavaTarget,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum CreatePlsqlKind {
    #[default]
    None,
    Procedure,
    Function,
    Package,
    PackageBody,
    TypeSpecAwaitingBody,
    TypeSpec,
    TypeBody,
    Trigger(TriggerKind),
    JavaSource,
    Wrapped,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum TriggerKind {
    Simple,
    Compound,
}

// ---------------------------------------------------------------------------

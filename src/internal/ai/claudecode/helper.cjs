const fs = require('fs');
const path = require('path');
const { createRequire } = require('module');
const readline = require('readline/promises');

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  return Buffer.concat(chunks).toString('utf8');
}

function collectProviderEnv(request) {
  const env = { ...process.env };
  const overrides = request.providerEnvOverrides;
  if (overrides !== undefined) {
    if (!overrides || typeof overrides !== 'object' || Array.isArray(overrides)) {
      throw new Error('providerEnvOverrides must be an object when present');
    }
    for (const [key, value] of Object.entries(overrides)) {
      if (typeof key !== 'string' || typeof value !== 'string') {
        throw new Error('providerEnvOverrides must contain only string pairs');
      }
      env[key] = value;
    }
  }

  const unset = request.providerEnvUnset;
  if (unset !== undefined) {
    if (!Array.isArray(unset)) {
      throw new Error('providerEnvUnset must be an array when present');
    }
    for (const key of unset) {
      if (typeof key !== 'string') {
        throw new Error('providerEnvUnset must contain only strings');
      }
      delete env[key];
    }
  }

  return env;
}

function findLastResultMessage(messages) {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    if (messages[index] && messages[index].type === 'result') {
      return messages[index];
    }
  }
  return null;
}

function stableNormalize(value) {
  if (Array.isArray(value)) {
    return value.map((item) => stableNormalize(item));
  }
  if (value && typeof value === 'object') {
    const normalized = {};
    for (const key of Object.keys(value).sort()) {
      if (value[key] !== undefined) {
        normalized[key] = stableNormalize(value[key]);
      }
    }
    return normalized;
  }
  return value;
}

function stableStringify(value) {
  return JSON.stringify(stableNormalize(value));
}

function shouldUseStreamMode(request) {
  return request.mode === 'queryStream' || request.stream === true;
}

function emitNdjsonEvent(enabled, type, payload = {}) {
  if (!enabled) {
    return;
  }
  process.stdout.write(`${JSON.stringify({ event: type, ...payload })}\n`);
}

function buildRuntimeSnapshot(hookEvents, messages, helperTimedOut, helperError) {
  const lastMessage = messages.length > 0 ? messages[messages.length - 1] : null;
  return {
    hookEventCount: hookEvents.length,
    messageCount: messages.length,
    helperTimedOut,
    helperError,
    lastMessageType: lastMessage && lastMessage.type ? lastMessage.type : null,
    lastMessageSubtype: lastMessage && lastMessage.subtype ? lastMessage.subtype : null,
    hasResultMessage: messages.some((message) => message && message.type === 'result'),
  };
}

function buildArtifact(request, hookEvents, messages, helperTimedOut, helperError) {
  return {
    cwd: request.cwd,
    prompt: request.prompt,
    requestContext: {
      enableFileCheckpointing: request.enableFileCheckpointing === true,
      interactiveApprovals: request.interactiveApprovals === true,
      continue: request.continue === true,
      resume: typeof request.resume === 'string' ? request.resume : null,
      forkSession: request.forkSession === true,
      sessionId: typeof request.sessionId === 'string' ? request.sessionId : null,
      resumeSessionAt: typeof request.resumeSessionAt === 'string' ? request.resumeSessionAt : null,
    },
    helperTimedOut,
    helperError,
    hookEvents,
    messages,
    resultMessage: findLastResultMessage(messages),
  };
}

function extractAssistantDelta(message) {
  if (!message || message.type !== 'stream_event') {
    return null;
  }
  const event = message.event;
  if (!event || event.type !== 'content_block_delta') {
    return null;
  }
  const delta = event.delta;
  if (!delta || delta.type !== 'text_delta' || typeof delta.text !== 'string') {
    return null;
  }
  return delta.text;
}

function buildHookInput(toolName, input, permissionOptions, extras = {}) {
  return {
    tool_name: toolName,
    tool_input: input,
    tool_use_id: permissionOptions && permissionOptions.toolUseID ? permissionOptions.toolUseID : null,
    agent_id: permissionOptions && permissionOptions.agentID ? permissionOptions.agentID : null,
    title: permissionOptions && permissionOptions.title ? permissionOptions.title : null,
    display_name:
      permissionOptions && permissionOptions.displayName ? permissionOptions.displayName : null,
    description:
      permissionOptions && permissionOptions.description ? permissionOptions.description : null,
    blocked_path:
      permissionOptions && permissionOptions.blockedPath ? permissionOptions.blockedPath : null,
    decision_reason:
      permissionOptions && permissionOptions.decisionReason ? permissionOptions.decisionReason : null,
    suggestions:
      permissionOptions && Array.isArray(permissionOptions.suggestions)
        ? permissionOptions.suggestions
        : [],
    ...extras,
  };
}

function hasAcceptEditsSuggestion(permissionOptions) {
  const suggestions =
    permissionOptions && Array.isArray(permissionOptions.suggestions)
      ? permissionOptions.suggestions
      : [];
  return suggestions.some(
    (suggestion) =>
      suggestion &&
      suggestion.type === 'setMode' &&
      suggestion.destination === 'session' &&
      suggestion.mode === 'acceptEdits'
  );
}

function loadScriptedResponses() {
  const raw = process.env.LIBRA_CLAUDE_HELPER_SCRIPTED_RESPONSES;
  if (!raw) {
    return [];
  }

  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    throw new Error(
      `failed to parse LIBRA_CLAUDE_HELPER_SCRIPTED_RESPONSES as JSON: ${error.message}`
    );
  }
  if (!Array.isArray(parsed)) {
    throw new Error('LIBRA_CLAUDE_HELPER_SCRIPTED_RESPONSES must be a JSON array');
  }
  return parsed.slice();
}

function hasInteractiveTty() {
  try {
    const readFd = fs.openSync('/dev/tty', 'r');
    const writeFd = fs.openSync('/dev/tty', 'w');
    fs.closeSync(readFd);
    fs.closeSync(writeFd);
    return true;
  } catch (_) {
    return false;
  }
}

function assertInteractiveInputAvailable(state) {
  if (state.scriptedResponses.length > 0) {
    return;
  }
  if (!hasInteractiveTty()) {
    throw new Error(
      'interactive approvals require an interactive terminal (/dev/tty); rerun in a terminal session or unset --interactive-approvals'
    );
  }
}

async function promptViaTty(lines, question) {
  let inputFd = null;
  let outputFd = null;
  try {
    inputFd = fs.openSync('/dev/tty', 'r');
    outputFd = fs.openSync('/dev/tty', 'w');
  } catch (_) {
    throw new Error(
      'interactive approvals require an interactive terminal (/dev/tty); rerun in a terminal session or unset --interactive-approvals'
    );
  }
  const input = fs.createReadStream(null, { fd: inputFd, autoClose: true });
  const output = fs.createWriteStream(null, { fd: outputFd, autoClose: true });
  let rl = null;

  try {
    for (const line of lines) {
      output.write(`${line}\n`);
    }
    rl = readline.createInterface({
      input,
      output,
      terminal: true,
    });
    const answer = await rl.question(question);
    return answer.trim();
  } finally {
    if (rl) {
      rl.close();
    }
    input.destroy();
    output.end();
  }
}

function buildApprovalCacheKey(toolName, input, permissionOptions) {
  return stableStringify({
    toolName,
    toolInput: input,
    blockedPath:
      permissionOptions && permissionOptions.blockedPath ? permissionOptions.blockedPath : null,
  });
}

function buildToolApprovalLines(toolName, input, permissionOptions) {
  const lines = ['', 'Tool approval required', `Tool: ${toolName}`];
  if (toolName === 'Bash') {
    if (input && input.command) {
      lines.push(`Command: ${input.command}`);
    }
    if (input && input.description) {
      lines.push(`Description: ${input.description}`);
    }
  } else {
    lines.push(`Input: ${JSON.stringify(input, null, 2)}`);
  }
  if (permissionOptions && permissionOptions.blockedPath) {
    lines.push(`Blocked path: ${permissionOptions.blockedPath}`);
  }
  if (permissionOptions && permissionOptions.decisionReason) {
    lines.push(`Reason: ${permissionOptions.decisionReason}`);
  }
  if (hasAcceptEditsSuggestion(permissionOptions)) {
    lines.push('Claude suggested switching this session to acceptEdits.');
  }
  return lines;
}

function parseToolApprovalDecision(answer, sessionUpgradeAvailable) {
  switch (answer.trim().toLowerCase()) {
    case 'a':
    case 'allow':
    case 'approve':
      return 'approve';
    case 's':
    case 'session':
    case 'switch':
    case 'switch-session':
    case 'switch_session':
    case 'switch-to-acceptedits':
    case 'switch_to_acceptedits':
    case 'approve-for-session':
    case 'approve_for_session':
      return sessionUpgradeAvailable ? 'switch_session' : 'approve_for_session';
    case 'd':
    case 'deny':
      return 'deny';
    case 'b':
    case 'abort':
      return 'abort';
    default:
      return null;
  }
}

function parseQuestionResponse(response, options, multiSelect) {
  const normalized = response.trim();
  if (!normalized) {
    return '';
  }
  const tokens = multiSelect ? normalized.split(',') : [normalized];
  const labels = tokens
    .map((token) => Number.parseInt(token.trim(), 10) - 1)
    .filter((index) => Number.isInteger(index) && index >= 0 && index < options.length)
    .map((index) => options[index].label);
  if (labels.length > 0) {
    return multiSelect ? labels.join(', ') : labels[0];
  }
  return normalized;
}

async function nextToolApprovalDecision(state, toolName, input, permissionOptions) {
  const sessionUpgradeAvailable = hasAcceptEditsSuggestion(permissionOptions);
  if (state.scriptedResponses.length > 0) {
    const scripted = state.scriptedResponses.shift();
    if (!scripted || scripted.kind !== 'tool_approval') {
      throw new Error('expected scripted tool_approval response');
    }
    return {
      decision:
        scripted.decision === 'switch_session' && !sessionUpgradeAvailable
          ? 'approve_for_session'
          : scripted.decision,
      promptSource: 'scripted',
    };
  }

  while (true) {
    const prompt = sessionUpgradeAvailable
      ? 'Choice [a]llow once/[s]witch session/[d]eny/a[b]ort: '
      : 'Choice [a]llow/[s]ession/[d]eny/a[b]ort: ';
    const answer = await promptViaTty(
      buildToolApprovalLines(toolName, input, permissionOptions),
      prompt
    );
    const decision = parseToolApprovalDecision(answer, sessionUpgradeAvailable);
    if (decision) {
      return {
        decision,
        promptSource: 'interactive_tty',
      };
    }
  }
}

async function collectAskUserQuestionAnswers(state, input) {
  if (state.scriptedResponses.length > 0) {
    const scripted = state.scriptedResponses.shift();
    if (!scripted || scripted.kind !== 'ask_user_question') {
      throw new Error('expected scripted ask_user_question response');
    }
    return {
      answers:
        scripted.answers && typeof scripted.answers === 'object' && !Array.isArray(scripted.answers)
          ? scripted.answers
          : {},
      promptSource: 'scripted',
    };
  }

  const answers = {};
  const questions = Array.isArray(input.questions) ? input.questions : [];
  for (let index = 0; index < questions.length; index += 1) {
    const question = questions[index];
    const options = Array.isArray(question.options) ? question.options : [];
    const key =
      typeof question.question === 'string' && question.question.trim().length > 0
        ? question.question
        : `question_${index + 1}`;
    const lines = ['', 'Agent question'];
    if (question.header) {
      lines.push(`${question.header}: ${question.question}`);
    } else {
      lines.push(key);
    }
    options.forEach((option, optionIndex) => {
      const description = option.description ? ` - ${option.description}` : '';
      lines.push(`  ${optionIndex + 1}. ${option.label}${description}`);
    });
    lines.push(
      question.multiSelect
        ? '  (Enter numbers separated by commas, or type your own answer)'
        : '  (Enter a number, or type your own answer)'
    );
    const response = await promptViaTty(lines, 'Your choice: ');
    answers[key] = parseQuestionResponse(response, options, question.multiSelect === true);
  }

  return {
    answers,
    promptSource: 'interactive_tty',
  };
}

function loadClaudeAgentSdk(cwd) {
  const attempts = [];
  const moduleOverride = process.env.LIBRA_CLAUDE_AGENT_SDK_MODULE;
  if (moduleOverride) {
    if (!path.isAbsolute(moduleOverride)) {
      throw new Error('LIBRA_CLAUDE_AGENT_SDK_MODULE must be an absolute module path');
    }
    if (!fs.existsSync(moduleOverride)) {
      throw new Error(`LIBRA_CLAUDE_AGENT_SDK_MODULE does not exist: ${moduleOverride}`);
    }
    attempts.push(() => require(moduleOverride));
  }

  const resolverCwd = path.resolve(cwd || process.cwd());
  attempts.push(() =>
    createRequire(path.join(resolverCwd, '__libra_claudecode__.cjs'))(
      '@anthropic-ai/claude-agent-sdk'
    )
  );
  attempts.push(() => require('@anthropic-ai/claude-agent-sdk'));

  let lastError = null;
  for (const attempt of attempts) {
    try {
      const sdk = attempt();
      if (sdk && typeof sdk.query === 'function') {
        return sdk;
      }
      lastError = new Error('resolved module does not export query()');
    } catch (error) {
      lastError = error;
    }
  }

  const detail = lastError && lastError.message ? lastError.message : String(lastError);
  throw new Error(
    'failed to load @anthropic-ai/claude-agent-sdk; install it in the working directory, configure NODE_PATH, or set LIBRA_CLAUDE_AGENT_SDK_MODULE. Last error: ' +
      detail
  );
}

function buildHooks(hookEvents, emitEvent, emitSnapshot) {
  const recordHook = (hookName) => async (input) => {
    hookEvents.push({ hook: hookName, input });
    switch (hookName) {
      case 'PermissionRequest':
        emitEvent('permission_request', { hook: hookName, input });
        break;
      case 'PreToolUse':
        emitEvent('tool_call', { hook: hookName, input });
        break;
      case 'PostToolUse':
      case 'PostToolUseFailure':
        emitEvent('tool_result', { hook: hookName, input });
        break;
      default:
        break;
    }
    emitSnapshot();
    return { continue: true };
  };

  const hookNames = [
    'SessionStart',
    'UserPromptSubmit',
    'PreToolUse',
    'PostToolUse',
    'PostToolUseFailure',
    'Notification',
    'SessionEnd',
    'Stop',
    'SubagentStart',
    'SubagentStop',
    'PreCompact',
    'PostCompact',
    'PermissionRequest',
    'Setup',
    'TeammateIdle',
    'TaskCompleted',
    'Elicitation',
    'ElicitationResult',
    'ConfigChange',
    'WorktreeCreate',
    'WorktreeRemove',
    'InstructionsLoaded',
  ];

  return Object.fromEntries(
    hookNames.map((hookName) => [hookName, [{ hooks: [recordHook(hookName)] }]])
  );
}

async function main() {
  const requestBody = await readStdin();
  if (!requestBody.trim()) {
    throw new Error('managed helper request is empty');
  }

  const request = JSON.parse(requestBody);
  const sdk = loadClaudeAgentSdk(request.cwd);
  if (request.mode === 'listSessions') {
    if (typeof sdk.listSessions !== 'function') {
      throw new Error('resolved @anthropic-ai/claude-agent-sdk does not export listSessions()');
    }
    const sessions = await sdk.listSessions({
      dir: request.cwd,
      limit: request.limit,
      offset: request.offset,
      includeWorktrees: request.includeWorktrees,
    });
    process.stdout.write(JSON.stringify(sessions));
    return;
  }
  if (request.mode === 'getSessionMessages') {
    if (typeof sdk.getSessionMessages !== 'function') {
      throw new Error(
        'resolved @anthropic-ai/claude-agent-sdk does not export getSessionMessages()'
      );
    }
    const messages = await sdk.getSessionMessages(request.providerSessionId, {
      limit: request.limit,
      offset: request.offset,
    });
    process.stdout.write(JSON.stringify(messages));
    return;
  }

  const streamMode = shouldUseStreamMode(request);
  const { query } = sdk;
  const hookEvents = [];
  const messages = [];
  let helperTimedOut = false;
  let helperError = null;
  const interactiveState = {
    scriptedResponses: loadScriptedResponses(),
    approvedToolCacheKeys: new Set(),
    sessionPermissionMode: request.permissionMode || 'default',
  };
  let liveQuery = null;
  const emitSnapshot = () =>
    emitNdjsonEvent(streamMode, 'runtime_snapshot', {
      snapshot: buildRuntimeSnapshot(hookEvents, messages, helperTimedOut, helperError),
      artifact: buildArtifact(request, hookEvents, messages, helperTimedOut, helperError),
    });
  const env = collectProviderEnv(request);

  const options = {
    cwd: request.cwd,
    model: request.model,
    settingSources: ['local', 'project', 'user'],
    env,
    permissionMode: request.permissionMode,
    hooks: buildHooks(
      hookEvents,
      (type, payload) => emitNdjsonEvent(streamMode, type, payload),
      emitSnapshot
    ),
    includePartialMessages: request.includePartialMessages === true,
    promptSuggestions: request.promptSuggestions === true,
    agentProgressSummaries: request.agentProgressSummaries === true,
  };
  if (request.enableFileCheckpointing === true) {
    options.enableFileCheckpointing = true;
  }

  if (request.continue === true) {
    options.continue = true;
  }
  if (typeof request.resume === 'string' && request.resume.length > 0) {
    options.resume = request.resume;
  }
  if (request.forkSession === true) {
    options.forkSession = true;
  }
  if (typeof request.sessionId === 'string' && request.sessionId.length > 0) {
    options.sessionId = request.sessionId;
  }
  if (typeof request.resumeSessionAt === 'string' && request.resumeSessionAt.length > 0) {
    options.resumeSessionAt = request.resumeSessionAt;
  }

  if (request.systemPrompt) {
    options.systemPrompt = request.systemPrompt;
  }

  if (Array.isArray(request.tools) && request.tools.length > 0) {
    options.tools = request.tools;
    if (request.interactiveApprovals === true) {
      assertInteractiveInputAvailable(interactiveState);
      options.canUseTool = async (toolName, input, permissionOptions) => {
        if (toolName === 'AskUserQuestion') {
          emitNdjsonEvent(streamMode, 'ask_user_question', {
            toolName,
            input,
          });
          const { answers, promptSource } = await collectAskUserQuestionAnswers(
            interactiveState,
            input
          );
          hookEvents.push({
            hook: 'CanUseTool',
            input: buildHookInput(toolName, input, permissionOptions, {
              interaction_kind: 'ask_user_question',
              prompt_source: promptSource,
              question_count: Array.isArray(input.questions) ? input.questions.length : 0,
              answer_count: Object.keys(answers).length,
              answers,
            }),
          });
          emitSnapshot();
          return {
            behavior: 'allow',
            updatedInput: {
              ...input,
              answers,
            },
          };
        }

        const cacheKey = buildApprovalCacheKey(toolName, input, permissionOptions);
        if (interactiveState.approvedToolCacheKeys.has(cacheKey)) {
          hookEvents.push({
            hook: 'CanUseTool',
            input: buildHookInput(toolName, input, permissionOptions, {
              interaction_kind: 'tool_approval',
              approval_decision: 'allow',
              approval_scope: 'session',
              prompt_source: 'session_cache',
              cached: true,
            }),
          });
          emitSnapshot();
          return { behavior: 'allow', updatedInput: input };
        }

        const { decision, promptSource } = await nextToolApprovalDecision(
          interactiveState,
          toolName,
          input,
          permissionOptions
        );

        if (decision === 'switch_session') {
          const previousMode = interactiveState.sessionPermissionMode;
          if (liveQuery && typeof liveQuery.setPermissionMode === 'function') {
            await liveQuery.setPermissionMode('acceptEdits');
            interactiveState.sessionPermissionMode = 'acceptEdits';
            hookEvents.push({
              hook: 'PermissionModeChanged',
              input: {
                previous_mode: previousMode,
                mode: 'acceptEdits',
                source: promptSource,
                tool_name: toolName,
                tool_input: input,
              },
            });
            emitNdjsonEvent(streamMode, 'permission_mode_changed', {
              previousMode,
              mode: 'acceptEdits',
              source: promptSource,
              toolName,
            });
            hookEvents.push({
              hook: 'CanUseTool',
              input: buildHookInput(toolName, input, permissionOptions, {
                interaction_kind: 'tool_approval',
                approval_decision: 'allow',
                approval_scope: 'session_mode',
                prompt_source: promptSource,
                cached: false,
                session_mode: 'acceptEdits',
              }),
            });
            emitSnapshot();
            return { behavior: 'allow', updatedInput: input };
          }

          interactiveState.approvedToolCacheKeys.add(cacheKey);
          hookEvents.push({
            hook: 'CanUseTool',
            input: buildHookInput(toolName, input, permissionOptions, {
              interaction_kind: 'tool_approval',
              approval_decision: 'allow',
              approval_scope: 'session',
              prompt_source: `${promptSource}_fallback_cache`,
              cached: false,
            }),
          });
          emitSnapshot();
          return { behavior: 'allow', updatedInput: input };
        }

        if (decision === 'approve_for_session') {
          interactiveState.approvedToolCacheKeys.add(cacheKey);
          hookEvents.push({
            hook: 'CanUseTool',
            input: buildHookInput(toolName, input, permissionOptions, {
              interaction_kind: 'tool_approval',
              approval_decision: 'allow',
              approval_scope: 'session',
              prompt_source: promptSource,
              cached: false,
            }),
          });
          emitSnapshot();
          return { behavior: 'allow', updatedInput: input };
        }

        if (decision === 'approve') {
          hookEvents.push({
            hook: 'CanUseTool',
            input: buildHookInput(toolName, input, permissionOptions, {
              interaction_kind: 'tool_approval',
              approval_decision: 'allow',
              approval_scope: 'request',
              prompt_source: promptSource,
              cached: false,
            }),
          });
          emitSnapshot();
          return { behavior: 'allow', updatedInput: input };
        }

        if (decision === 'abort') {
          hookEvents.push({
            hook: 'CanUseTool',
            input: buildHookInput(toolName, input, permissionOptions, {
              interaction_kind: 'tool_approval',
              approval_decision: 'abort',
              approval_scope: 'request',
              prompt_source: promptSource,
              cached: false,
            }),
          });
          emitSnapshot();
          return {
            behavior: 'deny',
            message: 'User aborted this action',
            interrupt: true,
          };
        }

        hookEvents.push({
          hook: 'CanUseTool',
          input: buildHookInput(toolName, input, permissionOptions, {
            interaction_kind: 'tool_approval',
            approval_decision: 'deny',
            approval_scope: 'request',
            prompt_source: promptSource,
            cached: false,
          }),
        });
        emitSnapshot();
        return { behavior: 'deny', message: 'User denied this action' };
      };
    } else if (request.autoApproveTools === true) {
      options.canUseTool = async (toolName, input, permissionOptions) => {
        hookEvents.push({
          hook: 'CanUseTool',
          input: buildHookInput(toolName, input, permissionOptions, {
            interaction_kind: 'tool_approval',
            approval_decision: 'allow',
            approval_scope: 'request',
            prompt_source: 'auto_approve',
            cached: false,
          }),
        });
        emitSnapshot();
        return { behavior: 'allow' };
      };
    } else {
      options.allowedTools = Array.isArray(request.allowedTools) ? request.allowedTools : request.tools;
    }
  }

  if (request.outputSchema) {
    options.outputFormat = {
      type: 'json_schema',
      schema: request.outputSchema,
    };
  }

  const stream = query({
    prompt: request.prompt,
    options,
  });
  liveQuery = stream;

  const iterator = stream[Symbol.asyncIterator]();
  let timeoutId = null;
  let timeoutPromise = null;
  if (typeof request.timeoutSeconds === 'number' && request.timeoutSeconds > 0) {
    timeoutPromise = new Promise((resolve) => {
      timeoutId = setTimeout(() => resolve({ __libraTimeout: true }), request.timeoutSeconds * 1000);
    });
  }

  try {
    while (true) {
      const nextMessage = iterator.next();
      const step = timeoutPromise ? await Promise.race([nextMessage, timeoutPromise]) : await nextMessage;
      if (step && step.__libraTimeout) {
        helperTimedOut = true;
        emitNdjsonEvent(streamMode, 'error', { error: 'helper_timed_out' });
        if (typeof iterator.return === 'function') {
          try {
            await iterator.return();
          } catch (_) {}
        }
        break;
      }
      if (!step || step.done) {
        break;
      }
      messages.push(step.value);
      emitNdjsonEvent(streamMode, 'sdk_message', { message: step.value });

      if (step.value && step.value.type === 'system' && step.value.subtype === 'init') {
        emitNdjsonEvent(streamMode, 'session_init', { message: step.value });
      }

      const assistantDelta = extractAssistantDelta(step.value);
      if (assistantDelta) {
        emitNdjsonEvent(streamMode, 'assistant_delta', {
          delta: assistantDelta,
          message: step.value,
        });
      }

      if (step.value && step.value.type === 'assistant') {
        emitNdjsonEvent(streamMode, 'assistant_message', { message: step.value });
      }

      if (step.value && step.value.type === 'result') {
        if (step.value.is_error === true || step.value.subtype === 'error') {
          emitNdjsonEvent(streamMode, 'error', { message: step.value });
        } else {
          emitNdjsonEvent(streamMode, 'result', { message: step.value });
        }
      }

      emitSnapshot();
    }
  } catch (error) {
    helperError = error && error.message ? error.message : String(error);
    emitNdjsonEvent(streamMode, 'error', { error: helperError });
  } finally {
    if (timeoutId) {
      clearTimeout(timeoutId);
    }
  }

  const artifact = buildArtifact(request, hookEvents, messages, helperTimedOut, helperError);
  if (streamMode) {
    emitSnapshot();
    emitNdjsonEvent(true, 'final_artifact', { artifact });
  } else {
    process.stdout.write(JSON.stringify(artifact));
  }
}

main().catch((error) => {
  const detail =
    error && error.stack ? error.stack : error && error.message ? error.message : String(error);
  process.stderr.write(detail);
  process.exitCode = 1;
});

const fs = require('fs');
const path = require('path');
const { createRequire } = require('module');

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  return Buffer.concat(chunks).toString('utf8');
}

function findLastResultMessage(messages) {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    if (messages[index] && messages[index].type === 'result') {
      return messages[index];
    }
  }
  return null;
}

function loadClaudeAgentSdk(cwd) {
  const attempts = [];
  const moduleOverride = process.env.LIBRA_CLAUDE_AGENT_SDK_MODULE;
  if (moduleOverride) {
    attempts.push(() => require(moduleOverride));
  }

  const resolverCwd = cwd || process.cwd();
  attempts.push(() =>
    createRequire(path.join(resolverCwd, '__libra_claude_sdk__.cjs'))(
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

function buildHooks(hookEvents) {
  const recordHook = (hookName) => async (input) => {
    hookEvents.push({ hook: hookName, input });
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
  const { query } = sdk;
  const hookEvents = [];
  const messages = [];
  let helperTimedOut = false;
  let helperError = null;

  const options = {
    cwd: request.cwd,
    model: request.model,
    settingSources: ['user'],
    permissionMode: request.permissionMode,
    hooks: buildHooks(hookEvents),
    includePartialMessages: request.includePartialMessages === true,
    promptSuggestions: request.promptSuggestions === true,
    agentProgressSummaries: request.agentProgressSummaries === true,
  };

  if (Array.isArray(request.tools) && request.tools.length > 0) {
    options.tools = request.tools;
    if (request.autoApproveTools) {
      options.canUseTool = async (toolName, input, permissionOptions) => {
        hookEvents.push({
          hook: 'CanUseTool',
          input: {
            tool_name: toolName,
            tool_input: input,
            tool_use_id: permissionOptions && permissionOptions.toolUseID ? permissionOptions.toolUseID : null,
            agent_id: permissionOptions && permissionOptions.agentID ? permissionOptions.agentID : null,
            blocked_path:
              permissionOptions && permissionOptions.blockedPath ? permissionOptions.blockedPath : null,
            decision_reason:
              permissionOptions && permissionOptions.decisionReason ? permissionOptions.decisionReason : null,
            suggestions:
              permissionOptions && Array.isArray(permissionOptions.suggestions)
                ? permissionOptions.suggestions
                : [],
          },
        });
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
    }
  } catch (error) {
    helperError = error && error.message ? error.message : String(error);
  } finally {
    if (timeoutId) {
      clearTimeout(timeoutId);
    }
  }

  const artifact = {
    cwd: request.cwd,
    prompt: request.prompt,
    helperTimedOut,
    helperError,
    hookEvents,
    messages,
    resultMessage: findLastResultMessage(messages),
  };

  process.stdout.write(JSON.stringify(artifact));
}

main().catch((error) => {
  const detail =
    error && error.stack ? error.stack : error && error.message ? error.message : String(error);
  process.stderr.write(detail);
  process.exitCode = 1;
});

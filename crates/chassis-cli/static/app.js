/* ==========================================================================
   Chassis Sovereign Dashboard - Client Application Logic
   ========================================================================== */

document.addEventListener('DOMContentLoaded', () => {
  // Navigation & Tab Switching
  const tabs = document.querySelectorAll('.nav-tab');
  const panes = document.querySelectorAll('.tab-pane');

  tabs.forEach(tab => {
    tab.addEventListener('click', () => {
      tabs.forEach(t => t.classList.remove('active'));
      panes.forEach(p => p.classList.remove('active'));

      tab.classList.add('active');
      const targetId = tab.getAttribute('data-tab');
      const targetPane = document.getElementById(targetId);
      if (targetPane) {
        targetPane.classList.add('active');
      }

      // Refresh data when switching to specific tabs
      if (targetId === 'tab-wal') fetchWalLedger();
      if (targetId === 'tab-plugins') fetchPluginsMatrix();
      if (targetId === 'tab-memory') fetchConversations();
    });
  });

  // Global State
  let currentSessionId = null;
  let eventSource = null;

  // Initialize Kernel Status & SSE
  initKernelStatus();
  initEventSource();

  // Quick Prompt Buttons
  document.querySelectorAll('.quick-prompt-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      const prompt = btn.getAttribute('data-prompt');
      const input = document.getElementById('input-goal');
      if (input) {
        input.value = prompt;
        input.focus();
      }
    });
  });

  // Clear Chat Stream
  const btnClearChat = document.getElementById('btn-clear-chat');
  if (btnClearChat) {
    btnClearChat.addEventListener('click', () => {
      const container = document.getElementById('chat-messages-container');
      const traceContainer = document.getElementById('trace-steps-container');
      if (container) {
        container.innerHTML = `
          <div class="system-welcome-card">
            <div class="card-icon">🛡️</div>
            <div class="card-content">
              <h3>Trajectory Cleared</h3>
              <p>Ready for next sovereign agent instruction.</p>
            </div>
          </div>
        `;
      }
      if (traceContainer) {
        traceContainer.innerHTML = `
          <div class="empty-placeholder">
            <span class="placeholder-icon">⚡</span>
            <p>Waiting for agent reasoning...</p>
          </div>
        `;
        document.getElementById('text-trace-count').textContent = '0 steps';
      }
    });
  }

  // Goal Form Submission
  const formChat = document.getElementById('form-chat');
  if (formChat) {
    formChat.addEventListener('submit', async (e) => {
      e.preventDefault();
      const input = document.getElementById('input-goal');
      const goal = input.value.trim();
      if (!goal) return;

      appendChatMessage('user', goal);
      input.value = '';

      setAgentState('Reasoning...', true);
      clearTraceInspector();

      try {
        const res = await fetch('/api/goal', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ goal, max_steps: 6 })
        });
        const data = await res.json();
        if (data.status === 'success') {
          appendChatMessage('agent', data.final_answer || 'Goal completed successfully.', data.trajectory);
          renderTrajectorySteps(data.trajectory || []);
        } else {
          appendChatMessage('agent', `⚠️ Error executing goal: ${data.message || 'Unknown error'}`);
        }
      } catch (err) {
        appendChatMessage('agent', `⚠️ Network/Kernel error: ${err.message}`);
      } finally {
        setAgentState('Idle', false);
        fetchWalLedger();
      }
    });
  }

  // RAG Search Form
  const formRag = document.getElementById('form-rag-search');
  if (formRag) {
    formRag.addEventListener('submit', async (e) => {
      e.preventDefault();
      const input = document.getElementById('input-rag-query');
      const query = input.value.trim();
      if (!query) return;

      const container = document.getElementById('rag-results-container');
      container.innerHTML = `<div class="empty-placeholder"><div class="spinner"></div><p>Searching DuckDuckGo & Wikipedia...</p></div>`;

      try {
        const res = await fetch('/api/search', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ query, limit: 5 })
        });
        const data = await res.json();
        renderRagResults(data.results || []);
      } catch (err) {
        container.innerHTML = `<div class="empty-placeholder"><p>Search error: ${err.message}</p></div>`;
      }
    });
  }

  // Vector Search Form
  const formVector = document.getElementById('form-vector-search');
  if (formVector) {
    formVector.addEventListener('submit', async (e) => {
      e.preventDefault();
      const input = document.getElementById('input-vector-query');
      const query = input.value.trim();
      if (!query) return;

      const container = document.getElementById('vector-results-container');
      container.innerHTML = `<div class="empty-placeholder"><div class="spinner"></div><p>Querying cosine ANN index...</p></div>`;

      try {
        const res = await fetch('/api/conversations/search', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ query, top_k: 5 })
        });
        const data = await res.json();
        renderVectorResults(data.matches || []);
      } catch (err) {
        container.innerHTML = `<div class="empty-placeholder"><p>Vector query error: ${err.message}</p></div>`;
      }
    });
  }

  // Refresh Buttons
  document.getElementById('btn-refresh-wal')?.addEventListener('click', fetchWalLedger);
  document.getElementById('btn-refresh-plugins')?.addEventListener('click', fetchPluginsMatrix);

  // --------------------------------------------------------------------------
  // Core API Functions
  // --------------------------------------------------------------------------

  async function initKernelStatus() {
    try {
      const res = await fetch('/api/status');
      const data = await res.json();
      currentSessionId = data.session_id;
      document.getElementById('text-session-id').textContent = data.session_id || 'ses_online';
      document.getElementById('text-wal-hash').textContent = (data.latest_hash || 'genesis').substring(0, 12) + '...';
      document.getElementById('stat-wal-latest-hash').textContent = data.latest_hash || 'Genesis';
      document.getElementById('stat-wal-events').textContent = data.event_count || '0';
      document.getElementById('badge-connection').className = 'status-badge';
      document.getElementById('text-connection').textContent = 'Kernel Online';
    } catch {
      document.getElementById('badge-connection').className = 'status-badge';
      document.getElementById('text-connection').textContent = 'Connecting...';
    }
  }

  function initEventSource() {
    try {
      eventSource = new EventSource('/api/events');
      eventSource.onmessage = (e) => {
        try {
          const evt = JSON.parse(e.data);
          handleLiveEvent(evt);
        } catch {
          // ignore parse errors
        }
      };
      eventSource.onerror = () => {
        // SSE auto reconnects
      };
    } catch (err) {
      console.warn('SSE not initialized:', err);
    }
  }

  function handleLiveEvent(evt) {
    if (evt.type === 'WAL_EVENT') {
      const hashEl = document.getElementById('text-wal-hash');
      if (hashEl && evt.hash) {
        hashEl.textContent = evt.hash.substring(0, 12) + '...';
      }
      const countEl = document.getElementById('stat-wal-events');
      if (countEl) {
        countEl.textContent = parseInt(countEl.textContent || '0') + 1;
      }
    } else if (evt.type === 'REACT_STEP') {
      appendTraceStep(evt.step);
    }
  }

  async function fetchWalLedger() {
    const tbody = document.getElementById('tbody-wal');
    if (!tbody) return;
    try {
      const res = await fetch('/api/wal');
      const data = await res.json();
      
      document.getElementById('stat-wal-events').textContent = data.entries?.length || 0;
      if (data.entries && data.entries.length > 0) {
        const last = data.entries[data.entries.length - 1];
        document.getElementById('stat-wal-latest-hash').textContent = last.entry_hash || 'Genesis';
        document.getElementById('text-wal-hash').textContent = (last.entry_hash || 'genesis').substring(0, 12) + '...';
      }

      if (!data.entries || data.entries.length === 0) {
        tbody.innerHTML = '<tr><td colspan="5" class="text-center">No WAL entries recorded yet.</td></tr>';
        return;
      }

      tbody.innerHTML = data.entries.map(e => `
        <tr>
          <td class="font-mono">${e.seq}</td>
          <td class="font-mono text-dim">${e.timestamp}</td>
          <td><span class="badge-accent">${e.event_type}</span></td>
          <td class="font-mono truncate" style="max-width: 320px;" title="${escapeHtml(JSON.stringify(e.payload))}">${escapeHtml(JSON.stringify(e.payload))}</td>
          <td class="font-mono truncate" style="max-width: 200px;" title="${e.entry_hash}">${e.entry_hash ? e.entry_hash.substring(0, 16) + '...' : 'Genesis'}</td>
        </tr>
      `).join('');
    } catch (err) {
      tbody.innerHTML = `<tr><td colspan="5" class="text-center">Error loading WAL: ${err.message}</td></tr>`;
    }
  }

  async function fetchPluginsMatrix() {
    const grid = document.getElementById('plugins-grid-container');
    if (!grid) return;
    try {
      const res = await fetch('/api/plugins');
      const data = await res.json();
      if (!data.plugins || data.plugins.length === 0) {
        grid.innerHTML = '<div class="empty-placeholder"><p>No active plugins registered in microkernel.</p></div>';
        return;
      }

      grid.innerHTML = data.plugins.map(p => `
        <div class="plugin-card">
          <div class="plugin-card-header">
            <div>
              <div class="plugin-id">${escapeHtml(p.id)}</div>
              <div class="plugin-version">v${escapeHtml(p.version || '0.1.0')} • ${p.mode || 'Isolated Process'}</div>
            </div>
            <span class="badge-success">Online</span>
          </div>
          
          <div>
            <div class="plugin-section-title">Capabilities Offered</div>
            <div class="plugin-caps-list">
              ${(p.capabilities_offered || []).map(c => `
                <div class="cap-item">
                  <span>⚡</span>
                  <span>${escapeHtml(c.id)}</span>
                  <span class="text-dim">(${c.methods?.join(', ') || ''})</span>
                </div>
              `).join('') || '<span class="text-dim">None (Consumer)</span>'}
            </div>
          </div>

          <div>
            <div class="plugin-section-title">Security & Permissions</div>
            <div class="plugin-badges">
              <span class="badge-accent">Zero Ambient Authority</span>
              ${p.permissions?.network ? '<span class="badge-amber">Network Whitelist</span>' : '<span class="badge-tool">No Network</span>'}
              ${p.permissions?.filesystem ? '<span class="badge-tool">Scoped FS</span>' : '<span class="badge-tool">No FS Access</span>'}
            </div>
          </div>
        </div>
      `).join('');
    } catch (err) {
      grid.innerHTML = `<div class="empty-placeholder"><p>Error fetching plugins: ${err.message}</p></div>`;
    }
  }

  async function fetchConversations() {
    const list = document.getElementById('conv-list-container');
    if (!list) return;
    try {
      const res = await fetch('/api/conversations');
      const data = await res.json();
      const countEl = document.getElementById('badge-conv-count');
      if (countEl) countEl.textContent = data.conversations?.length || 0;

      if (!data.conversations || data.conversations.length === 0) {
        list.innerHTML = '<div class="empty-placeholder"><p>No stored conversation threads in libSQL.</p></div>';
        return;
      }

      list.innerHTML = data.conversations.map(c => `
        <div class="conv-item-card">
          <div class="conv-item-header">
            <span class="conv-id">${escapeHtml(c.id)}</span>
            <span class="conv-date">${escapeHtml(c.created_at || '')}</span>
          </div>
          <div class="conv-title">${escapeHtml(c.title || 'Untitled Session')}</div>
        </div>
      `).join('');
    } catch (err) {
      list.innerHTML = `<div class="empty-placeholder"><p>Failed to load conversations: ${err.message}</p></div>`;
    }
  }

  // --------------------------------------------------------------------------
  // UI Render Helpers
  // --------------------------------------------------------------------------

  function appendChatMessage(sender, text, trajectory) {
    const container = document.getElementById('chat-messages-container');
    if (!container) return;

    const msg = document.createElement('div');
    msg.className = `chat-msg ${sender}`;

    let toolBadge = '';
    if (trajectory && trajectory.length > 0) {
      const toolsUsed = trajectory.filter(s => s.action).map(s => s.action.capability);
      if (toolsUsed.length > 0) {
        toolBadge = `<div style="margin-bottom: 8px; display: flex; gap: 4px; flex-wrap: wrap;">
          ${[...new Set(toolsUsed)].map(t => `<span class="badge-tool">🛠️ ${escapeHtml(t)}</span>`).join('')}
        </div>`;
      }
    }

    const time = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    msg.innerHTML = `
      <div class="chat-bubble">
        ${toolBadge}
        <div>${formatMarkdownish(text)}</div>
      </div>
      <span class="chat-time">${time}</span>
    `;

    container.appendChild(msg);
    container.scrollTop = container.scrollHeight;
  }

  function clearTraceInspector() {
    const container = document.getElementById('trace-steps-container');
    if (container) container.innerHTML = '';
    const countEl = document.getElementById('text-trace-count');
    if (countEl) countEl.textContent = '0 steps';
  }

  function appendTraceStep(step) {
    const container = document.getElementById('trace-steps-container');
    if (!container) return;

    const countEl = document.getElementById('text-trace-count');
    const curCount = container.querySelectorAll('.trace-step-card').length + 1;
    if (countEl) countEl.textContent = `${curCount} step${curCount > 1 ? 's' : ''}`;

    const card = document.createElement('div');
    card.className = 'trace-step-card';
    card.innerHTML = `
      <div class="step-card-header">
        <span class="step-index-pill">STEP #${step.step_number || curCount}</span>
        ${step.action ? `<span class="badge-tool">${escapeHtml(step.action.capability)}</span>` : '<span class="badge-accent">Thought</span>'}
      </div>
      ${step.thought ? `<div class="step-thought">💭 ${escapeHtml(step.thought)}</div>` : ''}
      ${step.action ? `
        <div class="step-action-box">
          <div class="step-action-title">⚙️ ACTION: ${escapeHtml(step.action.method)}</div>
          <div class="step-action-args">${escapeHtml(JSON.stringify(step.action.parameters, null, 2))}</div>
        </div>
      ` : ''}
      ${step.observation ? `
        <div class="step-obs-box">
          <div class="step-obs-title">👁️ OBSERVATION:</div>
          <div class="step-obs-text">${escapeHtml(step.observation)}</div>
        </div>
      ` : ''}
    `;

    container.appendChild(card);
    container.scrollTop = container.scrollHeight;
  }

  function renderTrajectorySteps(trajectory) {
    clearTraceInspector();
    trajectory.forEach(appendTraceStep);
  }

  function renderRagResults(results) {
    const container = document.getElementById('rag-results-container');
    if (!container) return;

    if (!results || results.length === 0) {
      container.innerHTML = '<div class="empty-placeholder"><p>No search results found.</p></div>';
      return;
    }

    container.innerHTML = results.map(r => `
      <div class="rag-item-card">
        <div class="rag-item-source">${escapeHtml(r.source || 'WEB')}</div>
        <div class="rag-item-title">
          <a href="${escapeHtml(r.url)}" target="_blank" rel="noopener">${escapeHtml(r.title)}</a>
        </div>
        <div class="rag-item-snippet">${escapeHtml(r.snippet)}</div>
      </div>
    `).join('');
  }

  function renderVectorResults(matches) {
    const container = document.getElementById('vector-results-container');
    if (!container) return;

    if (!matches || matches.length === 0) {
      container.innerHTML = '<div class="empty-placeholder"><p>No relevant semantic memory matches.</p></div>';
      return;
    }

    container.innerHTML = matches.map(m => `
      <div class="vector-match-card">
        <div class="vector-score-bar">
          <span class="conv-id">${escapeHtml(m.conversation_id || 'conv_system')}</span>
          <span class="similarity-score">Cosine: ${(m.score * 100).toFixed(1)}%</span>
        </div>
        <div class="conv-title">${escapeHtml(m.content)}</div>
      </div>
    `).join('');
  }

  function setAgentState(state, isBusy) {
    const badge = document.getElementById('badge-agent-state');
    const btn = document.getElementById('btn-dispatch-goal');
    const spinner = document.getElementById('spinner-goal');
    const btnText = document.getElementById('btn-goal-text');

    if (badge) {
      badge.textContent = state;
      badge.className = isBusy ? 'badge-amber' : 'badge-accent';
    }
    if (btn) btn.disabled = isBusy;
    if (spinner) spinner.classList.toggle('hidden', !isBusy);
    if (btnText) btnText.textContent = isBusy ? 'Executing...' : 'Execute Goal';
  }

  function escapeHtml(str) {
    if (typeof str !== 'string') return String(str || '');
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#039;');
  }

  function formatMarkdownish(str) {
    if (!str) return '';
    let escaped = escapeHtml(str);
    // Bold
    escaped = escaped.replace(/\*\*(.*?)\*\*/g, '<strong>$1</strong>');
    // Inline code
    escaped = escaped.replace(/`([^`]+)`/g, '<code class="font-mono">$1</code>');
    // Line breaks
    escaped = escaped.replace(/\n/g, '<br>');
    return escaped;
  }
});

'use strict';

const $ = id => document.getElementById(id);
let initialized = true;
let signedIn = false;
let selected = null;
let groups = [];
let noticeTimer;

function notify(message) {
  $('notification').textContent = message;
  $('notification').hidden = false;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => { $('notification').hidden = true; }, 6000);
}

async function api(path, body) {
  const response = await fetch(path, {
    method: body === undefined ? 'GET' : 'POST',
    credentials: 'same-origin',
    headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  let data;
  try { data = await response.json(); } catch { throw new Error('The server returned an unexpected response. Please try again.'); }
  if (!response.ok) {
    const error = new Error(data.error || `Request failed (${response.status}).`);
    error.status = response.status;
    throw error;
  }
  return data;
}

async function action(form, task) {
  const buttons = [...form.querySelectorAll('button')];
  buttons.forEach(button => { button.disabled = true; });
  try { await task(); } catch (error) { notify(error.message); }
  finally { buttons.forEach(button => { button.disabled = false; }); }
}

function auth() {
  signedIn = false;
  groups = [];
  selected = null;
  $('groups').replaceChildren();
  $('machines').replaceChildren();
  $('group-detail').hidden = true;
  $('loading').hidden = true;
  $('workspace').hidden = true;
  $('auth').hidden = false;
  $('password').autocomplete = initialized ? 'current-password' : 'new-password';
  $('auth-title').textContent = initialized ? 'Welcome back.' : 'Make yourself at home.';
  $('auth-description').textContent = initialized ? 'Sign in to manage your groups and machines.' : 'Create an account, then bring your own router and agents.';
  $('sign-in').textContent = initialized ? 'Sign in ↗' : 'Create account ↗';
}

async function workspace() {
  signedIn = true;
  initialized = true;
  $('loading').hidden = true;
  $('auth').hidden = true;
  $('workspace').hidden = false;
  $('password').value = '';
  await loadGroups();
}

function element(tag, className, text) {
  const item = document.createElement(tag);
  if (className) item.className = className;
  if (text !== undefined) item.textContent = text;
  return item;
}

async function allPages(path, key) {
  const records = [];
  let after = '00000000-0000-0000-0000-000000000000';
  for (;;) {
    const page = await api(`${path}?after=${encodeURIComponent(after)}`);
    records.push(...page);
    if (page.length < 100) return records;
    const next = page[page.length - 1][key];
    if (next <= after) throw new Error('The server returned an invalid page. Please refresh.');
    after = next;
  }
}

async function loadGroups() {
  groups = await allPages('/api/v1/groups', 'group_id');
  $('group-count').textContent = groups.length;
  $('agent-count').textContent = groups.reduce((sum, group) => sum + group.agents, 0);
  $('online-count').textContent = groups.reduce((sum, group) => sum + group.online, 0);
  $('empty').hidden = groups.length > 0;
  $('groups').replaceChildren();
  for (const group of groups) {
    const card = element('button', `group-card${selected === group.group_id ? ' selected' : ''}`);
    card.type = 'button';
    card.setAttribute('aria-pressed', String(selected === group.group_id));
    card.append(element('span', 'group-icon', '◈'), element('h3', '', group.name), element('p', 'group-id', group.group_id));
    const footer = element('footer');
    footer.append(element('span', '', `${group.agents} agent${group.agents === 1 ? '' : 's'}`), element('span', '', `${group.online} online ↗`));
    card.append(footer);
    card.addEventListener('click', () => { selected = group.group_id; loadGroups().catch(error => notify(error.message)); });
    $('groups').append(card);
  }
  const group = groups.find(item => item.group_id === selected);
  $('group-detail').hidden = !group;
  if (group) {
    $('detail-name').textContent = group.name;
    $('copy-group').textContent = `${group.group_id} ⧉`;
    await loadMachines();
  }
}

async function loadMachines() {
  const group = selected;
  const machines = await allPages(`/api/v1/groups/${group}/installations`, 'installation_id');
  if (group !== selected) return;
  $('machines').replaceChildren();
  if (!machines.length) {
    $('machines').append(element('p', 'muted', 'No machines yet. Connect your first agent to bring this group to life.'));
  }
  for (const machine of machines) {
    const row = element('article', 'machine-row');
    const identity = element('div');
    identity.append(element('h3', '', machine.name), element('p', '', machine.installation_id));
    const profile = element('div');
    const role = { router_agent: 'Router + agent', router: 'Router', agent: 'Agent' }[machine.role];
    profile.append(element('h3', '', role), element('p', '', machine.router_url || 'Connects to your group router'));
    row.append(identity, profile, element('span', `badge${machine.online ? '' : ' offline'}`, machine.online ? 'Ready' : 'Offline / starting'));
    const links = element('div');
    for (const [url, label] of [[machine.router_dashboard_url, 'Router dashboard ↗'], [machine.agent_dashboard_url, 'Agent dashboard ↗']]) {
      if (!url) continue;
      const link = element('a', 'text-button', label);
      link.href = url; link.target = '_blank'; link.rel = 'noopener noreferrer';
      links.append(link, document.createElement('br'));
    }
    row.append(links);
    $('machines').append(row);
  }
}

async function showPage(name) {
  for (const page of ['groups', 'guide']) $('page-' + page).hidden = page !== name;
  document.querySelectorAll('[data-page]').forEach(button => button.classList.toggle('active', button.dataset.page === name));
  $('breadcrumb').textContent = `Workspace / ${{ groups: 'Agent groups', guide: 'Installation guide' }[name]}`;
  if (name === 'groups') await loadGroups();

}

$('login-form').addEventListener('submit', event => {
  event.preventDefault();
  action(event.currentTarget, async () => {
    const input = { username: $('username').value, password: $('password').value };
    await api(`/api/v1/auth/${initialized ? 'login' : 'register'}`, input);
    await workspace();
  });
});
$('logout').addEventListener('click', () => action($('workspace'), async () => { await api('/api/v1/auth/logout', {}); auth(); }));
document.querySelectorAll('[data-page]').forEach(button => button.addEventListener('click', () => showPage(button.dataset.page).catch(error => notify(error.message))));
for (const id of ['new-group', 'empty-create']) $(id).addEventListener('click', () => $('group-dialog').showModal());
document.querySelectorAll('[data-close]').forEach(button => button.addEventListener('click', () => $(button.dataset.close).close()));
$('invite-dialog').addEventListener('close', () => { $('invite-code').value = ''; });
$('group-form').addEventListener('submit', event => {
  event.preventDefault();
  action(event.currentTarget, async () => {
    const result = await api('/api/v1/groups', { name: $('group-name').value.trim() });
    selected = result.group_id;
    $('group-dialog').close();
    $('group-form').reset();
    await loadGroups();
    notify('Group created. Connect a machine to get started.');
  });
});
$('refresh').addEventListener('click', () => loadGroups().catch(error => notify(error.message)));
$('copy-group').addEventListener('click', () => copy(selected, 'Group ID copied.'));
$('copy-install').addEventListener('click', () => copy($('install-command').value, 'Install command copied.'));
$('copy-invite').addEventListener('click', () => copy($('invite-code').value, 'Enrollment code copied.'));
async function copy(value, message) { try { await navigator.clipboard.writeText(value); notify(message); } catch { notify('Copy unavailable. Select and copy the value manually.'); } }
$('add-machine').addEventListener('click', () => action($('group-detail'), async () => {
  const result = await api(`/api/v1/groups/${selected}/invitations`, {});
  $('invite-origin').value = location.origin;
  const platform = "'" + location.origin.replaceAll("'", "'\"'\"'") + "'";
  $('install-command').value = "curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/Code-Mount-Labs/cuttlefish/v0.2.0-alpha.1/install.sh | sh -s -- --platform " + platform;
  $('invite-group').value = selected;
  $('invite-code').value = result.invitation;
  $('invite-dialog').showModal();
}));
$('auth-toggle').addEventListener('click', () => { initialized = !initialized; auth(); $('auth-toggle').textContent = initialized ? 'New here? Create an account' : 'Already have an account? Sign in'; });
setInterval(() => { if (signedIn && !document.hidden && !$('page-groups').hidden && !document.querySelector('dialog[open]')) loadGroups().catch(error => { if (error.status === 401) auth(); else notify(error.message); }); }, 15000);

(async () => {
  try {
    await api('/api/v1/status');
    try { await api('/api/v1/auth/session'); await workspace(); }
    catch (error) { if (error.status === 401) auth(); else throw error; }
  } catch (error) { $('loading').querySelector('p').textContent = 'Unable to reach this workspace. Refresh to try again.'; notify(error.message); }
})();

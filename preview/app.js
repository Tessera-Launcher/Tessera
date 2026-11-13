'use strict';
(() => {
  const $ = selector => document.querySelector(selector);
  const escape = value => String(value).replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[char]));
  const icons = {
    home: '<path d="m3 10 9-7 9 7v10H3Z"/><path d="M9 20v-7h6v7"/>',
    accounts: '<circle cx="12" cy="8" r="4"/><path d="M4 21v-2a8 8 0 0 1 16 0v2"/>',
    download: '<path d="M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5"/>',
    java: '<rect x="5" y="5" width="14" height="14" rx="2"/><path d="M9 1v4m6-4v4M9 19v4m6-4v4M1 9h4m-4 6h4m14-6h4m-4 6h4M9 9h6v6H9Z"/>',
    music: '<path d="M9 18V5l11-2v13M9 9l11-2"/><ellipse cx="6" cy="18" rx="3" ry="3"/><ellipse cx="17" cy="16" rx="3" ry="3"/>',
    box: '<path d="m12 2 10 5v10l-10 5L2 17V7Z M2 7l10 5 10-5M12 12v10M7 4.5l10 5"/>',
    check: '<path d="m5 12 4 4L19 6"/>',
    arrow: '<path d="m9 5 7 7-7 7"/>',
  };
  const icon = name => `<svg class="icon" viewBox="0 0 24 24" aria-hidden="true">${icons[name] || icons.box}</svg>`;
  const pages = [['home', '主页', '1'], ['accounts', '账户', '2'], ['download', '下载', '3'], ['java', '运行时', '4'], ['music', '音乐', '5']];
  const versions = [
    { id: '1.21.4', type: 'release', java: '28 EA', size: '47.2 MiB', aot: true },
    { id: '1.21.4-fabric-0.16.10', type: 'release', java: '21', size: '52.8 MiB', loader: 'Fabric' },
    { id: '1.20.1-forge-47.3.0', type: 'release', java: '17', size: '68.4 MiB', loader: 'Forge' },
    { id: '1.18.2', type: 'release', java: '28 EA', size: '19.6 MiB' },
    { id: '1.8.9', type: 'release', java: '28 EA', size: '8.1 MiB', aot: true },
    { id: '25w02a', type: 'snapshot', java: '28 EA', size: '28.5 MiB' },
    { id: 'b1.7.3', type: 'legacy', java: '28 EA', size: '1.4 MiB' },
    { id: '1.16.5-incomplete', type: 'broken', java: '17', size: '—' },
  ];
  const catalog = ['1.21.4', '1.21.3', '1.21.2', '1.21.1', '1.21', '1.20.6', '1.20.4', '1.20.1', '1.19.4', '1.18.2', '1.8.9'].map(id => ({ id, type: 'release' })).concat([{ id: '25w02a', type: 'snapshot' }, { id: '24w46a', type: 'snapshot' }, { id: 'b1.7.3', type: 'legacy' }, { id: 'a1.2.6', type: 'legacy' }]);
  const runtimes = [{ major: 28, version: '28-ea+20', size: '61.8 MiB', ea: true }, { major: 25, version: '25.0.1+8', size: '56.2 MiB' }, { major: 21, version: '21.0.9+10', size: '50.4 MiB', installed: true }, { major: 17, version: '17.0.17+10', size: '46.8 MiB', installed: true }, { major: 8, version: '8u472-b08', size: '39.1 MiB' }];
  const songs = [{ title: 'C418 — Sweden', duration: 215, size: '3.8 MiB' }, { title: 'C418 — Minecraft', duration: 254, size: '4.6 MiB' }, { title: 'C418 — Subwoofer Lullaby', duration: 208, size: '3.7 MiB' }, { title: 'C418 — Wet Hands', duration: 90, size: '1.6 MiB' }, { title: 'C418 — Living Mice', duration: 177, size: '3.2 MiB' }, { title: 'C418 — Haggstrom', duration: 204, size: '3.6 MiB' }];
  const records = [{ title: 'C418 — cat', duration: 185, size: '3.3 MiB' }, { title: 'C418 — blocks', duration: 345, size: '6.1 MiB' }, { title: 'Lena Raine — Pigstep', duration: 148, size: '2.7 MiB' }];
  const state = { page: 'home', scenario: 'normal', selected: 0, accounts: [{ name: 'Alex', application: 'Microsoft', offline: false }, { name: 'Steve', application: 'Offline', offline: true }], account: 0, accountPick: 0, confirmAccount: false, filter: 'all', query: '', download: 0, loader: '原版', source: 'BMCLAPI', install: null, java: 0, javaSource: '清华镜像', javaProgress: null, login: 'choose', loginError: '', returnPage: 'accounts', musicVersion: 0, musicKind: 'bgm', track: 0, playing: false, position: 63, volume: 70, order: 0 };
  const selectedVersion = () => versions[state.selected];
  const tracks = () => state.musicKind === 'bgm' ? songs : records;
  const clock = seconds => `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, '0')}`;
  const button = (text, action, style = '', attrs = '') => `<button class="${style}" data-action="${action}" ${attrs}>${escape(text)}</button>`;
  const badge = (text, type = '') => `<span class="badge ${type}">${escape(text)}</span>`;
  const panel = (title, body, classes = '', meta = '') => `<section class="panel ${classes}"><div class="panel-heading"><h2>${escape(title)}</h2>${meta}</div>${body}</section>`;
  const header = (title, description, meta = '') => `<header class="page-header"><div><h1>${escape(title)}</h1><p>${escape(description)}</p></div>${meta}</header>`;
  const footer = (status, actions = '') => `<footer class="footer"><span class="status" role="status">${escape(status)}</span><div class="actions">${actions}</div></footer>`;
  function empty(title, description, error = false) { return `<div class="empty"><div class="empty-icon ${error ? 'error' : ''}">${icon(error ? 'java' : 'box')}</div><h3 class="${error ? 'error' : ''}">${escape(title)}</h3><p class="caption">${escape(description)}</p>${error ? button('重新加载', 'retry') : ''}</div>`; }
  function scenario(defaultContent, emptyText = '这里还没有内容', loadingText = '正在加载…') {
    if (state.scenario === 'empty') return empty(emptyText, '选择其他页面或切回「默认」状态查看示例。');
    if (state.scenario === 'loading') return empty(loadingText, '正在预览等待状态。切回「默认」即可继续。');
    if (state.scenario === 'error') return empty('暂时无法加载', '连接超时，请稍后重试。这里展示的是错误状态。', true);
    return defaultContent;
  }
  function sidebar() {
    const account = state.accounts[state.account];
    return `<aside class="sidebar"><div class="brand"><div class="mark" aria-hidden="true"><i></i><i></i><i></i><i></i></div><div><div class="brand-name">Tessera</div><small>Minecraft 启动器</small></div></div><div><div class="nav-label">工作空间</div><nav aria-label="主导航">${pages.map(([id, label, key]) => `<button class="nav-item ${state.page === id ? 'active' : ''}" data-action="navigate" data-page="${id}" ${state.page === id ? 'aria-current="page"' : ''}>${icon(id)}<span>${label}</span><kbd>${key}</kbd></button>`).join('')}</nav></div><div class="sidebar-foot"><div class="account-summary"><div class="avatar">${escape(account?.name[0] || '?')}</div><div><div>${escape(account?.name || '尚未登录')}</div><small class="muted">${escape(account?.application || '添加一个账户开始游戏')}</small></div></div></div></aside>`;
  }
  function home() {
    const v = selectedVersion();
    const rows = versions.map((version, i) => `<button class="row ${state.selected === i ? 'selected' : ''}" data-action="version" data-index="${i}" aria-pressed="${state.selected === i}"><i class="dot ${version.type}"></i><span class="name">${escape(version.id)}</span>${version.type === 'broken' ? badge('缺少文件', 'broken') : ''}<small class="optional">Java ${version.java}</small></button>`).join('');
    const fields = v ? [['目录', v.id], ['类型', v.loader || ({ release: '正式版', snapshot: '快照', legacy: '历史版本', broken: '不完整' }[v.type])], ['Java', v.java], ['AOT Cache', v.aot ? '已就绪' : '未训练'], ['主类', v.loader ? 'KnotClient / Bootstrap' : 'net.minecraft.client.main.Main'], ['继承', '—'], ['客户端 jar', v.type === 'broken' ? '缺失' : '已就绪'], ['大小', v.size]] : [];
    const details = `<div class="panel-body"><dl class="fields">${fields.map(([key, value]) => `<dt>${key}</dt><dd>${escape(value)}</dd>`).join('')}</dl></div>`;
    const logs = `<div class="panel-body log scroll grow"><p><time>10:42:08</time>Tessera 已就绪</p><p><time>10:42:08</time>扫描 data/游戏/versions</p><p><time>10:42:08</time>发现 ${versions.length} 个本地版本</p><p><time>10:42:09</time>已载入 ${state.accounts.length} 个账户</p><p><time>10:42:09</time><span class="accent">选择一个版本，开始新的旅程。</span></p></div>`;
    return header('主页', '你的世界，从这里出发。', `<span class="eyebrow">${versions.length} 个版本 · ${state.accounts.length} 个账户</span>`) + `<div class="content">${panel('本地版本', scenario(`<div class="panel-body list scroll grow">${rows || empty('没有找到版本', '前往下载页安装第一个版本。')}</div>`, '没有找到任何版本'), 'grow', button('＋ 下载', 'go-download', 'quiet'))}<div class="stack details">${panel('版本详情', scenario(details, '未选择版本'))}${panel('活动日志', logs, 'grow', badge('就绪'))}</div></div>` + footer(v ? `已选择 ${v.id} · Enter 启动 · 右键查看更多操作` : '前往下载页添加版本', button('刷新', 'refresh') + button('开始 AOT 训练', 'aot') + button('启动游戏  →', 'launch', 'primary', !v || state.scenario !== 'normal' ? 'disabled' : ''));
  }
  function accounts() {
    const rows = state.accounts.map((account, i) => `<button class="row tall ${state.accountPick === i ? 'selected' : ''}" data-action="account" data-index="${i}" aria-pressed="${state.accountPick === i}"><span class="avatar">${escape(account.name[0])}</span><span class="name">${escape(account.name)}<span class="subtitle">${account.offline ? '离线账户 · 仅单人游戏' : 'Microsoft 账户 · 已保存的会话'}</span></span>${state.account === i ? badge('✓ 当前账户') : ''}<small>${escape(account.application)}</small></button>`).join('');
    return header('账户', '管理已保存的会话，随时切换你的身份。', badge(`${state.accounts.length} 个账户`)) + `<div class="content">${panel('已保存的会话', scenario(`<div class="panel-body list scroll grow">${rows || empty('还没有账户', '点击「登录新账户」添加一个。')}</div>`, '还没有账户'), 'grow')}</div>` + footer(state.confirmAccount ? '再次点击「确认移除」删除所选示例账户' : 'Enter 或双击切换账户 · 移除需要确认', button('登录新账户', 'login') + button(state.confirmAccount ? '确认移除' : '移除', 'remove-account', 'danger', !state.accounts.length ? 'disabled' : '') + button('切换账户', 'activate-account', 'primary', !state.accounts.length ? 'disabled' : ''));
  }
  function versionResults() {
    const shown = catalog.map((v, i) => ({ ...v, index: i })).filter(v => (state.filter === 'all' || v.type === state.filter) && v.id.toLowerCase().includes(state.query.toLowerCase()));
    return shown.map(v => `<button class="row ${state.download === v.index ? 'selected' : ''}" data-action="download-version" data-index="${v.index}" aria-pressed="${state.download === v.index}"><i class="dot ${v.type}"></i><span class="name">${v.id}</span><small>${({ release: '正式版', snapshot: '快照', legacy: '历史版本' })[v.type]}</small></button>`).join('') || empty('没有匹配的版本', '试试其他版本号或筛选条件。');
  }
  function download() {
    if (state.install) return installation();
    const filters = [['all', '全部'], ['release', '正式版'], ['snapshot', '快照'], ['legacy', '历史']];
    const toolbar = `<div class="search-toolbar"><input id="version-search" type="search" placeholder="搜索版本，例如 1.21 / 1.8.9" aria-label="搜索版本" value="${escape(state.query)}"><div class="filters">${filters.map(([value, label]) => button(label, 'filter', state.filter === value ? 'active' : '', `data-value="${value}" aria-pressed="${state.filter === value}"`)).join('')}</div></div>`;
    const loaders = ['原版', 'Fabric', 'Forge', 'NeoForge', 'Quilt', 'LiteLoader'].map(name => `<button class="row ${state.loader === name ? 'selected' : ''}" data-action="loader" data-value="${name}" aria-pressed="${state.loader === name}"><i class="dot"></i><span class="name">${name}</span>${state.loader === name ? icon('check') : ''}</button>`).join('');
    return header('下载', '选择游戏版本，再搭配你喜欢的加载器。', badge('JAVA EDITION')) + `<div class="content">${panel('Minecraft 版本', toolbar + `<div id="version-results" class="panel-body list scroll grow">${scenario(versionResults(), '没有可安装的版本', '正在获取版本列表…')}</div>`, 'grow')}<div class="stack inspector">${panel('安装内容', `<div class="panel-body list">${loaders}</div>`, 'grow')}${panel('下载来源', `<div class="panel-body"><p>${state.source}</p><p class="muted caption">仅影响本次运行</p></div>`)}</div></div>` + footer(`已选择 ${catalog[state.download].id} · ${state.loader}`, button('切换下载源', 'source') + button('安装版本  ↓', 'install', 'primary', state.scenario !== 'normal' ? 'disabled' : ''));
  }
  function installation() {
    const job = state.install;
    return header(job.percent >= 100 ? '安装完成' : '正在安装', `${job.version} · ${job.loader}`, badge('演示进度')) + `<div class="content stack">${panel('活动', `<div class="panel-body log grow scroll"><p><time>10:43:00</time>读取版本信息 ${escape(job.version)}</p><p><time>10:43:01</time>准备客户端与依赖库</p><p><time>10:43:02</time>校验资源文件</p><p class="accent">${job.percent >= 100 ? '演示安装完成，已添加到示例版本列表。' : '演示下载正在进行，切换页面后会保留进度。'}</p></div>`, 'grow')}${panel('进度', `<div class="progress-box"><div class="progress-label"><strong class="display" id="install-percent">${job.percent}%</strong><span class="muted caption" id="install-files">${Math.round(job.percent * 18.6)} / 1860 个文件</span></div><div class="progress" role="progressbar" aria-label="安装进度" aria-valuenow="${job.percent}" aria-valuemin="0" aria-valuemax="100"><span id="install-bar" style="width:${job.percent}%"></span></div><small class="muted">示例速度 12.8 MiB/s · 未下载真实文件</small></div>`)}</div>` + footer(job.percent >= 100 ? '可以返回版本列表查看' : '浏览其他页面时，演示进度会继续', button('返回版本列表', 'install-back'));
  }
  function java() {
    const list = runtimes.map((runtime, i) => `<button class="row ${state.java === i ? 'selected' : ''}" data-action="runtime" data-index="${i}" aria-pressed="${state.java === i}"><i class="dot"></i><span class="name">Java ${runtime.major}<span class="subtitle">${runtime.version}</span></span>${runtime.ea ? badge('EA', 'legacy') : ''}${runtime.installed ? badge('已安装') : ''}<small>${runtime.size}</small></button>`).join('');
    const installed = runtimes.filter(x => x.installed).map(runtime => `<div class="row"><span class="name">Java ${runtime.major}<span class="subtitle">${runtime.version}</span></span>${icon('check')}</div>`).join('');
    const progress = state.javaProgress;
    return header('Java 运行时', '为不同的游戏版本准备合适的运行环境。', badge('Adoptium')) + `<div class="content">${panel('可用运行时', scenario(`<div class="panel-body list scroll">${list}</div>`, '没有可用运行时', '正在查询 Adoptium…'), 'grow')}<div class="stack inspector">${panel('已安装', `<div class="panel-body list">${installed}</div>`, 'grow')}${panel('下载与来源', `<div class="progress-box"><p class="caption" id="java-progress-text">${progress ? `演示下载 Java ${progress.major} · ${progress.percent}%` : '当前没有下载任务'}</p><div class="progress"><span id="java-bar" style="width:${progress?.percent || 0}%"></span></div><small class="muted">${state.javaSource}</small>${state.javaSource === 'GitHub' ? '<label class="caption muted" for="proxy">GitHub Proxy</label><input id="proxy" placeholder="例如 gh-proxy.com（可留空）">' : ''}</div>`)}</div></div>` + footer('运行时独立保存在 data/运行时', button('切换下载源', 'java-source') + button('全部更新', 'java-update') + button('下载运行时', 'java-install', 'primary', state.scenario !== 'normal' || progress?.percent < 100 ? 'disabled' : ''));
  }
  function music() {
    const collection = tracks();
    const versionList = versions.filter(v => v.type !== 'broken').map((version, i) => `<button class="row ${state.musicVersion === i ? 'selected' : ''}" data-action="music-version" data-index="${i}" aria-pressed="${state.musicVersion === i}"><span class="name">${escape(version.id)}</span></button>`).join('');
    const rows = collection.map((track, i) => `<button class="row ${state.track === i ? 'selected' : ''}" data-action="track" data-index="${i}" aria-pressed="${state.track === i}"><span class="track-number">${String(i + 1).padStart(2, '0')}</span><span class="name">${escape(track.title)}</span><small>${clock(track.duration)}</small><small class="optional">${track.size}</small></button>`).join('');
    return header('音乐', '回到熟悉的方块世界，听见那些记忆。', badge('仅演示播放控件')) + `<div class="content"><div class="stack music-versions">${panel('游戏版本', scenario(`<div class="panel-body list scroll">${versionList}</div>`, '没有已安装版本'), 'grow')}</div>${panel(`曲目 (${collection.length})`, scenario(`<div class="panel-body list scroll">${rows}</div>`, '没有可播放曲目'), 'grow')}</div><div class="transport"><div class="filters">${button('唱片', 'music-kind', state.musicKind === 'records' ? 'active' : '', 'data-value="records"')}${button('背景音乐', 'music-kind', state.musicKind === 'bgm' ? 'active' : '', 'data-value="bgm"')}</div>${button(state.playing ? 'Ⅱ' : '▶', 'play', '', `id="play" aria-label="${state.playing ? '暂停' : '播放'}"`)}<span id="position" class="clock">${clock(state.position)}</span><input class="seek" id="seek" type="range" min="0" max="${collection[state.track].duration}" value="${state.position}" aria-label="播放进度"><span class="clock">${clock(collection[state.track].duration)}</span><label for="volume" class="caption muted">音量</label><input class="volume" id="volume" type="range" min="0" max="100" value="${state.volume}" aria-label="音量">${button(['顺序播放', '单曲循环', '随机播放'][state.order], 'order')}</div>`;
  }
  function login() {
    let body = '';
    if (state.login === 'choose') body = `<div class="list">${[['official', '官方启动器', '使用 Microsoft 账户登录 Minecraft'], ['custom', '自定义 app id', '使用 Azure 应用的客户端身份'], ['offline', '离线登录', '使用本地身份进入单人游戏']].map(([value, title, subtitle]) => `<button class="row tall" data-action="login-method" data-value="${value}">${icon(value === 'offline' ? 'home' : 'accounts')}<span class="name">${title}<span class="subtitle">${subtitle}</span>${icon('arrow')}</button>`).join('')}</div>`;
    if (state.login === 'offline') body = '<form id="offline-form" class="form"><div class="callout">离线模式不创建 Microsoft 会话，可用于单人游戏。</div><label for="username">玩家名称<input id="username" name="username" placeholder="例如 Steve" autocomplete="off" required></label><label class="check"><input type="checkbox" name="allowAny">允许任意用户名（中文、空格等）</label><p class="error caption" id="login-error" role="alert"></p><button type="submit" class="primary">离线登录</button></form>';
    if (state.login === 'custom') body = '<form id="custom-form" class="form"><label for="client-id">Azure 应用（客户端）ID<input id="client-id" name="clientId" placeholder="00000000-0000-0000-0000-000000000000" required></label><p class="muted caption">输入一个有效 UUID 后，预览设备授权界面。</p><p class="error caption" id="login-error" role="alert"></p><button type="submit" class="primary">继续登录</button></form>';
    if (state.login === 'device') body = `<div class="form"><div class="callout">这是设备授权界面的示例。下方设备码无效，请勿提交到 Microsoft。</div><p class="muted caption">在浏览器中输入设备码，授权后返回启动器。</p><div class="device-code">DEMO-2026</div><div class="actions">${button('复制示例代码', 'copy-code')}${button('模拟授权成功', 'login-success', 'primary')}${button('模拟失败', 'login-failure')}</div><small class="muted">等待浏览器授权…</small></div>`;
    if (state.login === 'done') body = empty('登录成功', '示例账户已添加，可以返回账户页查看。');
    if (state.login === 'failed') body = `<div class="form"><p class="error">授权已过期，请重新登录。</p><p class="muted caption">已保留其他账户，你可以重新发起授权。</p>${button('重新选择登录方式', 'login-retry', 'primary')}</div>`;
    return header('登录', '选择你的身份，连接下一个世界。', badge('账户')) + `<div class="content">${panel('身份 / 等待授权', `<div class="panel-body grow">${body}</div>`, 'grow')}</div>` + footer('预览中的账户仅保存在当前页面内存中', (state.login !== 'choose' ? button('上一步', 'login-retry') : '') + button('关闭', 'close-login'));
  }
  function render() {
    const renderers = { home, accounts, download, java, music, login };
    $('#app').innerHTML = (state.page === 'login' ? '' : sidebar()) + `<main class="main ${state.page === 'login' ? 'login-main' : ''}" id="main">${renderers[state.page]()}</main>`;
    document.title = `Tessera · ${pages.find(page => page[0] === state.page)?.[1] || '登录'}预览`;
  }
  let toastTimer;
  function notify(text) { $('#toast').textContent = text; $('#toast').hidden = false; clearTimeout(toastTimer); toastTimer = setTimeout(() => { $('#toast').hidden = true; }, 3800); }
  function navigate(page) { state.page = page; $('#context-menu').hidden = true; render(); }
  function normal() { state.scenario = 'normal'; $('#scenario').value = 'normal'; }
  function showDialog(body) { $('#dialog-content').innerHTML = body; $('#dialog').showModal(); }
  function launch() {
    if (!selectedVersion()) return;
    if (!state.accounts.length) { state.returnPage = 'home'; state.login = 'choose'; navigate('login'); return; }
    if (selectedVersion().type === 'broken') { notify('此版本缺少客户端文件，请重新安装。'); return; }
    notify(`预览：已选择以 ${state.accounts[state.account].name} 启动 ${selectedVersion().id}。网页不会启动游戏。`);
  }
  function startJava() { if (state.javaProgress && state.javaProgress.percent < 100) return; const runtime = runtimes[state.java]; state.javaProgress = { major: runtime.major, percent: 0 }; normal(); render(); }
  function activateAccount() { if (!state.accounts.length) return; state.account = state.accountPick; state.confirmAccount = false; render(); notify(`已切换到 ${state.accounts[state.account].name}`); }
  const actions = {
    navigate: el => navigate(el.dataset.page),
    'go-download': () => { normal(); navigate('download'); },
    version: el => { state.selected = Number(el.dataset.index); render(); },
    refresh: () => { normal(); render(); notify('已重新载入示例列表。'); },
    retry: () => { normal(); render(); },
    aot: () => notify('预览：AOT 训练需要桌面程序和 Java 28 EA。'),
    launch,
    account: el => { state.accountPick = Number(el.dataset.index); state.confirmAccount = false; render(); },
    'activate-account': activateAccount,
    'remove-account': () => { if (!state.accounts.length) return; if (state.confirmAccount) { const removed = state.accountPick; state.accounts.splice(removed, 1); if (state.account > removed) state.account--; else if (state.account === removed) state.account = 0; state.accountPick = Math.min(removed, Math.max(0, state.accounts.length - 1)); state.confirmAccount = false; notify('示例账户已移除。'); } else state.confirmAccount = true; render(); },
    login: () => { state.returnPage = state.page; state.login = 'choose'; navigate('login'); },
    'close-login': () => navigate(state.returnPage),
    'login-method': el => { state.login = el.dataset.value === 'official' ? 'device' : el.dataset.value; render(); },
    'login-retry': () => { state.login = 'choose'; render(); },
    'login-success': () => { state.accounts.push({ name: 'Alex Preview', application: 'Microsoft · 示例', offline: false }); state.account = state.accounts.length - 1; state.accountPick = state.account; state.login = 'done'; render(); },
    'login-failure': () => { state.login = 'failed'; render(); },
    'copy-code': async () => { try { await navigator.clipboard.writeText('DEMO-2026'); notify('已复制无效示例代码 DEMO-2026。'); } catch { notify('浏览器未允许复制，请手动选择 DEMO-2026。'); } },
    filter: el => { state.filter = el.dataset.value; render(); },
    'download-version': el => { state.download = Number(el.dataset.index); render(); },
    loader: el => { state.loader = el.dataset.value; render(); },
    source: () => { state.source = state.source === 'BMCLAPI' ? 'Mojang 官方' : 'BMCLAPI'; render(); },
    install: () => { if (state.install) return; state.install = { version: catalog[state.download].id, loader: state.loader, percent: 0 }; render(); },
    'install-back': () => { if (state.install.percent >= 100) state.install = null; else { navigate('home'); return; } render(); },
    runtime: el => { state.java = Number(el.dataset.index); render(); },
    'java-source': () => { state.javaSource = state.javaSource === '清华镜像' ? 'GitHub' : '清华镜像'; render(); },
    'java-install': startJava,
    'java-update': () => { notify('演示更新所选运行时。'); startJava(); },
    'music-version': el => { state.musicVersion = Number(el.dataset.index); render(); },
    'music-kind': el => { state.musicKind = el.dataset.value; state.track = 0; state.position = 0; state.playing = false; render(); },
    track: el => { state.track = Number(el.dataset.index); state.position = 0; render(); },
    play: () => { state.playing = !state.playing; render(); if (state.playing) notify('仅演示播放进度，不输出音频。'); },
    order: () => { state.order = (state.order + 1) % 3; render(); },
    uninstall: () => { if (!selectedVersion()) return; showDialog(`<h2 id="dialog-title">卸载 ${escape(selectedVersion().id)}？</h2><p>会从当前预览移除这个示例版本。你的磁盘文件不会被修改。</p><p class="caption warning">桌面程序会在这里列出受影响的版本与回收大小。</p><div class="actions">${button('取消', 'close-dialog')}${button('确认卸载', 'confirm-uninstall', 'danger')}</div>`); },
    'confirm-uninstall': () => { versions.splice(state.selected, 1); state.selected = Math.min(state.selected, Math.max(0, versions.length - 1)); $('#dialog').close(); render(); notify('已从预览移除这个版本。'); },
    'close-dialog': () => $('#dialog').close(),
    tokens: () => { const colors = [['background-canvas', '画布'], ['background-surface', '面板'], ['fill-primary', '主要操作'], ['text-on-fill', '填充上的文字'], ['background-selected', '选中'], ['fill-warning', '快照'], ['status-danger', '错误'], ['text-secondary', '次要文字']]; const css = getComputedStyle(document.documentElement); showDialog(`<h2 id="dialog-title">Tessera · 语义配色</h2><p>4 个关键色通过统一 OKHSL 色阶生成浅色与深色主题。组件按 background、text、fill、border、status 角色取色。</p><div class="swatches">${colors.map(([key, label]) => `<div class="swatch"><i style="background:var(--color-${key})"></i>${label}<small class="mono">${escape(css.getPropertyValue('--color-' + key).trim())}</small></div>`).join('')}</div><p class="caption">正文 14 px · 辅助文字 12 px · 标题 21 px<br>间距 2 / 4 / 8 / 12 / 16 / 24 / 32 / 48 px<br>圆角 3 / 6 / 12 px</p><div class="actions">${button('关闭', 'close-dialog', 'primary')}</div>`); },
  };
  document.addEventListener('click', event => {
    const target = event.target.closest('[data-action]');
    if (target && !target.disabled) actions[target.dataset.action]?.(target);
    if (target || !event.target.closest('#context-menu')) $('#context-menu').hidden = true;
  });
  document.addEventListener('input', event => {
    if (event.target.id === 'version-search') { state.query = event.target.value; $('#version-results').innerHTML = scenario(versionResults()); }
    if (event.target.id === 'seek') { state.position = Number(event.target.value); $('#position').textContent = clock(state.position); }
    if (event.target.id === 'volume') state.volume = Number(event.target.value);
  });
  $('#scenario').addEventListener('change', event => { state.scenario = event.target.value; render(); });
  document.addEventListener('submit', event => {
    if (!['offline-form', 'custom-form'].includes(event.target.id)) return;
    event.preventDefault();
    const data = new FormData(event.target);
    if (event.target.id === 'custom-form') {
      if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(String(data.get('clientId')).trim())) { $('#login-error').textContent = '请输入有效的 Azure 客户端 UUID。'; return; }
      state.login = 'device'; render(); return;
    }
    const name = String(data.get('username')).trim();
    if (!name || (!data.has('allowAny') && !/^[A-Za-z0-9_]{3,16}$/.test(name))) { $('#login-error').textContent = '用户名需要 3–16 个字母、数字或下划线；或勾选「允许任意用户名」。'; return; }
    state.accounts.push({ name, application: 'Offline', offline: true }); state.account = state.accounts.length - 1; state.accountPick = state.account; state.login = 'done'; render();
  });
  document.addEventListener('contextmenu', event => {
    const row = event.target.closest('[data-action="version"]');
    if (!row) return;
    event.preventDefault(); state.selected = Number(row.dataset.index); render();
    const menu = $('#context-menu'); menu.innerHTML = button('启动游戏', 'launch', '', 'role="menuitem"') + button('卸载…', 'uninstall', 'danger', 'role="menuitem"') + button('刷新', 'refresh', '', 'role="menuitem"'); menu.hidden = false;
    menu.style.left = Math.min(event.clientX, innerWidth - menu.offsetWidth - 12) + 'px'; menu.style.top = Math.min(event.clientY, innerHeight - menu.offsetHeight - 12) + 'px'; menu.querySelector('button').focus();
  });
  document.addEventListener('keydown', event => {
    if ($('#dialog').open) return;
    if (event.key === 'Escape') { $('#context-menu').hidden = true; if (state.page === 'login') navigate(state.returnPage); else if (state.page !== 'home') { if (state.page === 'music') state.playing = false; navigate('home'); } return; }
    if (event.target.matches('input, select, textarea') || event.ctrlKey || event.metaKey || event.altKey) return;
    if (/^[1-5]$/.test(event.key)) { navigate(pages[Number(event.key) - 1][0]); return; }
    if (state.page === 'home' && versions.length) {
      if (['ArrowDown', 'ArrowUp', 'j', 'k'].includes(event.key)) { event.preventDefault(); state.selected = Math.max(0, Math.min(versions.length - 1, state.selected + (['ArrowDown', 'j'].includes(event.key) ? 1 : -1))); render(); }
      if (event.key === 'Delete') actions.uninstall();
      if (event.key === 'Enter' && !event.target.closest('button')) launch();
    }
    if (state.page === 'music' && event.code === 'Space' && !event.target.closest('button')) { event.preventDefault(); actions.play(); }
  });
  // Keep click targets in place for a second click, despite reactive rendering.
  let lastClick = { action: '', index: '', time: 0 };
  document.addEventListener('click', event => {
    const el = event.target.closest('[data-action]');
    if (!el || !['version', 'account', 'track'].includes(el.dataset.action)) return;
    const now = performance.now();
    if (lastClick.action === el.dataset.action && lastClick.index === el.dataset.index && now - lastClick.time < 350) {
      if (el.dataset.action === 'version') launch();
      if (el.dataset.action === 'account') activateAccount();
      if (el.dataset.action === 'track') { state.playing = true; render(); }
      lastClick.time = 0;
    } else lastClick = { action: el.dataset.action, index: el.dataset.index, time: now };
  });
  setInterval(() => {
    if (state.install && state.install.percent < 100) {
      state.install.percent = Math.min(100, state.install.percent + 8);
      if (state.install.percent === 100) {
        const job = state.install;
        const id = job.version + (job.loader === '原版' ? '' : '-' + job.loader.toLowerCase());
        if (!versions.some(v => v.id === id)) versions.push({ id, type: 'release', java: job.loader === '原版' ? '28 EA' : '21', size: '48.2 MiB', loader: job.loader === '原版' ? undefined : job.loader });
        notify('演示安装完成。');
        if (state.page === 'download') render();
      } else if ($('#install-bar')) {
        $('#install-bar').style.width = state.install.percent + '%'; $('#install-percent').textContent = state.install.percent + '%'; $('#install-files').textContent = `${Math.round(state.install.percent * 18.6)} / 1860 个文件`; $('#install-bar').parentElement.setAttribute('aria-valuenow', state.install.percent);
      }
    }
    if (state.javaProgress && state.javaProgress.percent < 100) {
      state.javaProgress.percent = Math.min(100, state.javaProgress.percent + 10);
      if (state.javaProgress.percent === 100) { runtimes.find(r => r.major === state.javaProgress.major).installed = true; if (state.page === 'java') render(); notify('运行时演示下载完成。'); }
      else if ($('#java-bar')) { $('#java-bar').style.width = state.javaProgress.percent + '%'; $('#java-progress-text').textContent = `演示下载 Java ${state.javaProgress.major} · ${state.javaProgress.percent}%`; }
    }
    if (state.playing) {
      state.position += .5;
      if (state.position >= tracks()[state.track].duration) { state.position = 0; if (state.order === 0) state.track = (state.track + 1) % tracks().length; if (state.order === 2) state.track = Math.floor(Math.random() * tracks().length); if (state.page === 'music') render(); }
      if ($('#position')) { $('#position').textContent = clock(state.position); if (document.activeElement !== $('#seek')) $('#seek').value = state.position; }
    }
  }, 500);
  render();
})();

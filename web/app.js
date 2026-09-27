'use strict';
const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => [...document.querySelectorAll(selector)];
const token = $('meta[name="yourself-token"]').content;
const names = {chat:'对话',memories:'记忆',activity:'活动记录',settings:'设置'};
const kinds = {task_update:['任务进展','完成、阻塞或提醒到期时联系你'],personal_discovery:['研究发现','完成你安排的探索后分享发现'],curiosity_question:['好奇提问','基于已记录的信息缺口提问'],social_check_in:['轻量问候','你明确允许时才主动联系']};
const jobKinds = {self_review:'自主整理',autonomous_message:'主动消息候选',knowledge_search:'公开百科检索',cold_start:'冷启动整理',task:'文字工作',research:'研究探索',reflection:'对话反思',reminder:'定时提醒',curiosity:'好奇提问',check_in:'轻量问候'};
const states = {queued:'等待执行',running:'正在处理',done:'已完成',failed:'未完成',cancelled:'已取消',pending:'等待回复',unknown:'状态未知',skipped:'Jev 选择等待'};
const reasons = {jev_action:'Jev 判定当前行动',weighted_appraisal:'候选具有新增价值，通过评估',cooldown:'仍在冷却期',daily_budget:'已达到主动联系配额',kind_not_authorized:'该类主动联系未授权',quiet_or_busy:'免打扰或忙碌中',no_candidate:'没有需要主动告知的候选',user_initiated:'用户主动对话，交给对话模型',cloud_not_authorized:'外部调用未授权',paused:'服务已暂停',semantic_gate:'价值或打扰风险未达到要求',evidence_not_ready:'证据不足，等待核实',decision_unavailable:'评估失败，未主动联系',state_changed_during_decision:'上下文已变化，放弃旧结果',candidate_already_evaluated:'候选已经处理'};
let data=null, view='chat', messageSignature='', listSignatures={}, settingsLoaded=false, polling=false, sending=false, toastTimer;
const escape = (value) => String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
function rich(text){return String(text??'').split(/(https?:\/\/[^\s<>"）)]+)/g).map((part,i)=>i%2?`<a href="${escape(part)}" target="_blank" rel="noopener noreferrer">${escape(part)}</a>`:escape(part)).join('');}
const time = (ms) => new Date(ms).toLocaleString('zh-CN',{month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit',hour12:false});
const money = (value) => value == null ? '费用未提供' : `$${Number(value).toFixed(5)}`;
function toast(text,error=false){const el=$('#toast');el.textContent=text;el.classList.toggle('error',error);el.hidden=false;clearTimeout(toastTimer);toastTimer=setTimeout(()=>el.hidden=true,error?6500:3500);}
async function api(path, options={}) {
  const response=await fetch(`/api/${path}`,{...options,headers:{'X-Yourself-Token':token,'Content-Type':'application/json',...options.headers}});
  let body;try{body=await response.json();}catch{throw new Error(`服务返回异常（${response.status}），请刷新页面后重试。`);}
  if(!response.ok)throw new Error(body.error||`请求失败（${response.status}）`);
  return body;
}
const post=(path,body={})=>api(path,{method:'POST',body:JSON.stringify(body)});
async function action(button, fn){const old=button?.disabled;if(button)button.disabled=true;try{await fn();}catch(e){toast(e.message,true);}finally{if(button)button.disabled=old;}}
function navigate(next){if(!names[next])next='chat';view=next;location.hash=next;$$('.view').forEach(el=>el.hidden=el.id!==`view-${next}`);$$('[data-view]').forEach(el=>{const active=el.dataset.view===next;el.classList.toggle('active',active);if(active)el.setAttribute('aria-current','page');else el.removeAttribute('aria-current');});$('#page-name').textContent=names[next];if(next==='settings'&&data&&!settingsLoaded)loadSettings();renderLists();}
$$('[data-view]').forEach(el=>el.addEventListener('click',()=>navigate(el.dataset.view)));
$$('[data-go]').forEach(el=>el.addEventListener('click',()=>navigate(el.dataset.go)));
window.addEventListener('hashchange',()=>navigate(location.hash.slice(1)));
async function refresh(){if(polling)return;polling=true;try{data=await api('state');$('#service-error').hidden=true;render();}catch(e){$('#service-error').textContent=`连接本地服务失败：${e.message}`;$('#service-error').hidden=false;$('#connection-status').textContent='服务连接中断';}finally{polling=false;}}
function render(){
  if(!data)return;
  $('#drive-energy').textContent=`联系历史：上次用户回复后主动发送 ${data.drive?.energy?.unanswered_proactive??0} 条 · 无固定发送上限`;
  const focus=data.attention||{};
  $('#attention-status').textContent=['long_term','medium_term','short_term'].map((k,i)=>`${['长期','中期','短期'][i]}关注强度 ${Number(focus[k]?.strength??60).toFixed(1)} · 粘性 ${focus[k]?.stickiness??'—'}`).join('；');
  $('#learned-skills').textContent=`已沉淀 Skills：${(data.skills||[]).map(s=>`${s.name} v${s.version} — ${s.description}`).join('；')||'暂无，主循环可从真实经验中学习'}`;
  const goal=data.drive?.goal||{};
  $('#drive-goal').textContent=['long_term','medium_term','short_term'].map((k,i)=>`${['长期','中期','短期'][i]}：${goal[k]||'尚未形成'}`).join('\n\n');
  $('#workspace-path').textContent=data.workspace||'';
  $('#browser-tool-status').textContent=data.browser_ready?'Browser · 独立无头浏览器已启用':'Browser · 暂未启用：系统通知权限已确认，嵌套沙箱兼容性验证未通过。';
  const config=data.settings, pause=config.guards.paused;
  const providerName=config.provider==='teamorouter'?'TeamoRouter':'OpenRouter';
  const connected=false;
  $('#connection-status').textContent=pause?'已暂停':!config.has_key?`等待连接 ${providerName}`:!config.guards.cloud_allowed?'外部调用已关闭':connected?`${providerName} 已连接`:`${providerName} · Key 已保存`;
  $('#connection-status').classList.toggle('ready',config.has_key&&!pause&&connected);
  $('#setup-banner').hidden=config.has_key;
  const blocked=!config.has_key||pause||!config.guards.cloud_allowed;
  $('#send').disabled=blocked||data.thinking||sending;
  $('#stop-chat').hidden=!data.thinking;
  $('#composer-hint').textContent=!config.has_key?'先连接模型，就可以开始对话':pause?'服务已暂停，恢复后即可继续':data.thinking?'知微正在处理你的消息…':'Enter 发送 · Shift + Enter 换行';

  const a=data.mind.affect;const affect=[['心境',(a.valence+1)/2,a.valence>0.15?'积极':a.valence<-.15?'低落':'平和'],['活跃度',a.arousal,a.arousal>.55?'活跃':'平稳'],['把握感',a.control,a.control>.65?'清晰':'适中']];
  $('#affect').innerHTML=affect.map(([label,value,word])=>`<div class="affect-row"><div class="affect-label"><span>${label}</span><span>${word}</span></div><div class="affect-track"><progress aria-label="${label}" max="1" value="${value}"></progress></div></div>`).join('');
  $('#recent-work').innerHTML=data.jobs.length?data.jobs.slice(0,3).map(j=>`<div class="recent-job">${escape(j.objective.slice(0,60))}<small>${states[j.status]} · ${time(j.created_at)}</small></div>`).join(''):'还没有后台任务。<br>一个清晰的目标，就是起点。';
  $('#usage-summary').textContent=`${data.usage.calls} 次调用 · ${data.usage.tokens.toLocaleString()} Token${data.usage.cost!=null?' · '+money(data.usage.cost):''}`;
  renderMessages();renderLists();if(view==='settings'&&!settingsLoaded)loadSettings();
}
function renderMessages(){
  const signature=JSON.stringify(data.messages);if(signature===messageSignature)return;messageSignature=signature;
  const container=$('#messages');const nearBottom=window.scrollY+innerHeight>=document.documentElement.scrollHeight-220;
  if(!data.messages.length){container.innerHTML=`<div class="empty-chat"><h2>你好，我是知微。<br>今天，从什么聊起？</h2><p>聊一个还没想清楚的问题，分享你的计划，或者告诉我一件值得记住的小事。</p><div class="suggestions"><button class="suggestion" data-suggestion="我有一个想法，想和你一起理清楚。">一起理清一个想法</button><button class="suggestion" data-suggestion="请记住：我喜欢简洁、直接、有依据的回答。">告诉你我的偏好</button><button class="suggestion" data-suggestion="你目前能帮我做哪些事？">了解知微的能力</button></div></div>`;return;}
  container.innerHTML=data.messages.map(m=>`<article class="message ${m.role==='user'?'user':'assistant'}" data-id="${escape(m.id)}"><div class="avatar" aria-hidden="true">${m.role==='user'?'我':'知'}</div><div class="message-body"><div class="message-label">${m.role==='user'?'你':'知微'}${m.mode==='proactive'?'<span class="tag">主动联系</span>':''}<time>${time(m.created_at)}</time></div>${m.status==='pending'?`${m.content?`<div class="message-content">${rich(m.content)}</div>`:''}<div class="thinking-label">${m.content?'正在回复':'正在思考'}<span class="thinking-dots">...</span></div>`:m.status==='failed'?`<div class="message-error">${escape(m.error||'本次生成未完成。')}<button class="text-button" data-retry-message="${escape(m.reply_to||'')}">重新发送</button></div>`:`<div class="message-content">${rich(m.content)}</div>`}</div></article>`).join('');
  if(nearBottom&&view==='chat')requestAnimationFrame(()=>window.scrollTo({top:document.documentElement.scrollHeight,behavior:'auto'}));
}
function renderLists(){if(!data)return;
  if(view==='memories'){
    const key=JSON.stringify(data.memories);if(listSignatures.memories===key)return;listSignatures.memories=key;
    const dimensions={expression:'表达详略',initiative:'主动程度',evidence:'求证倾向',disagreement:'不同意见的表达'};
    const values={concise:'简洁',balanced:'适中',detailed:'详细',reserved:'克制',proactive:'积极',exploratory:'探索',verify_first:'先核实',gentle:'温和',direct:'直接'};
    function profileText(p){return Object.entries(p.preferences||{}).map(([k,v])=>`${dimensions[k]||k}：${values[v]||v}`).join(' · ');}
    $('#adaptive-profiles').innerHTML=['user','self'].map(subject=>{const p=data.profiles?.[subject]||{};return `<h3>${subject==='user'?'对你的理解':'知微的自我人格'}</h3><p>${escape(profileText(p))}</p><p class="field-help">${p.version?'Jev 根据对话更新 · '+time(p.updated_at):'初始设定，尚未形成新的偏好版本'}</p>${Object.entries(p.observations||{}).map(([k,v])=>`<p class="field-help">待观察：${escape(dimensions[k])} → ${escape(values[v.value])}（${v.count} 次依据）</p>`).join('')}`;}).join('')+'<p class="field-help">临时观察不改变生效偏好。当前对话要求优先，联系节奏由 Jev 根据记忆与反馈判断。下方保留版本及对话依据；删除旧版本也会清除依赖它形成的后续版本。</p>';
    $('#memories-summary').textContent=`${data.memories.length} 条记忆`;
    const labels={self_experience:'自身经历与反馈',self_belief:'自身理解（待验证）',profile_user:'用户理解版本',profile_self:'自我人格版本',user_note:'你告诉知微的事',reflection:'有依据的反思',source_knowledge:'来源知识',belief:'暂定理解',plan:'尚未发生的计划'};
    $('#memories-list').innerHTML=data.memories.length?data.memories.map(m=>`<article class="memory-item"><div class="item-top"><div class="item-meta"><span class="status-tag">${labels[m.kind]||escape(m.kind)}${m.superseded?' · 已被新理解替代':''}</span><time>${time(m.created_at)}</time></div><div>${m.kind.startsWith("profile_")?`<button class="text-button" data-restore-profile="${escape(m.id)}">恢复此版本</button>`:''}<button class="text-button danger" data-forget="${escape(m.id)}">删除</button></div></div><div class="memory-content">${rich(m.kind.startsWith("profile_")?(()=>{try{const p=JSON.parse(m.content);return profileText(p)+"\n对话依据："+(p.evidence_excerpt||"");}catch{return m.content;}})():m.content)}</div>${m.source?`<p class="field-help">来源：${escape(m.source)}</p>`:''}</article>`).join(''):'<div class="empty-state"><strong>给记忆留一点空间</strong>添加一条偏好或约定。知微会在之后的对话中参考它。</div>';
  }
  if(view==='activity'){
    const u=data.usage;$('#activity-usage').innerHTML=`<span><strong>${u.calls}</strong>次调用 / 24h</span><span><strong>${u.tokens.toLocaleString()}</strong>Token</span><span><strong>${money(u.cost)}</strong>已记录费用${u.unpriced?' · 部分调用未返回费用':''}</span>`;
    const nextLabels={reply:'回应',wait:'等待',search_memory:'查记忆',remember:'保存记忆',create_task:'创建任务',reflect:'内部反思',notify:'主动通知'};
    $('#jev-plans').innerHTML=(data.plans||[]).map(p=>`<div class="audit-row"><span class="status-tag">Jev</span><div>${escape(nextLabels[p.plan.next]||p.plan.next)}${p.plan.kind?' · '+escape(kinds[p.plan.kind]?.[0]||p.plan.kind):''}<div class="audit-detail">${p.phase==='dialogue'?'对话 / 工具结果':'后台新成果'}</div></div><time>${time(p.created_at)}</time></div>`).join('');
    $('#decisions-list').innerHTML=data.decisions.length?data.decisions.map(d=>`<div class="audit-row"><span class="status-tag">${{speak:'建议联系',respond:'直接回复',wait:'等待',reflect:'需要反思',stale:'丢弃旧结果',duplicate:'重复事件'}[d.action]||escape(d.action)}</span><div>${escape(reasons[d.reason]||d.reason)}<div class="audit-detail">${escape(d.event_id)} · ${escape(d.policy_version)}</div></div><span class="audit-number">${d.utility?d.utility.toFixed(3):''}</span></div>`).join(''):'<div class="empty-state">发生对话或任务事件后，判断记录会出现在这里。</div>';
    renderTraces();
  }
}
function loadSettings(){const s=data.settings;$('#primary-channel').value=s.primary_channel||'feishu';$('#provider').value=s.provider||'openrouter';$('#api-key').value='';$('#api-key').placeholder=s.has_key?'已保存，留空表示保持不变':'sk-or-v1-…';$('#key-status').textContent=s.has_key?'已保存':'尚未配置';$('#model').value=s.model;$('#decision-model').value=s.decision_model;$('#persona-notes').value=s.persona_notes;$('#max-tokens').value=s.max_tokens;$('#cloud-allowed').checked=s.guards.cloud_allowed;$('#web-search').checked=s.web_search;$('#exploration-goal').value=s.exploration_goal||'';$('#exploration-interval').value=s.exploration_interval_minutes||0;$('#remove-key').hidden=!s.has_key;
  loadProvider();settingsLoaded=true;}
function collectSettings(){return {primary_channel:$('#primary-channel').value,provider:$('#provider').value,api_key:$('#api-key').value.trim()||null,model:$('#model').value.trim(),decision_model:$('#decision-model').value.trim(),persona_notes:$('#persona-notes').value,max_tokens:Number($('#max-tokens').value),daily_call_limit:0,web_search:$('#web-search').checked,exploration_goal:$('#exploration-goal').value.trim(),exploration_interval_minutes:Number($('#exploration-interval').value),
  guards:{...data.settings.guards,cloud_allowed:$('#cloud-allowed').checked}};}
async function saveSettings(test=false){if(!$('#settings-form').reportValidity())return;const button=test?$('#save-test'):$('#settings-form button[type=submit]');await action(button,async()=>{const payload=collectSettings();$('#settings-status').textContent='正在保存设置…';try{await post('settings',payload);$('#api-key').value='';settingsLoaded=false;await refresh();loadSettings();if(test){$('#settings-status').textContent='正在向所选模型发送一条简短测试…';const result=await post('test');$('#settings-status').textContent=`连接成功 · ${result.model}${result.jev_tested?' · Jev 判定已验证':''}`;toast('连接成功，可以回到对话开始使用。');await refresh();}else{$('#settings-status').textContent='设置已保存。';toast('设置已保存');}}catch(e){$('#settings-status').textContent=e.message;throw e;}});}
function loadProvider(){const provider=$('#provider').value;const profile=data.settings.profiles[provider];const teamo=provider==='teamorouter';$('#api-key').value='';$('#api-key').placeholder=profile.has_key?'已保存，留空表示保持不变':teamo?'sk-teamo-…':'sk-or-v1-…';$('#key-status').textContent=profile.has_key?'已保存':'尚未配置';$('#model').value=profile.model;$('#decision-model').value=profile.decision_model;$('#remove-key').hidden=!profile.has_key;$('#models').replaceChildren();$('#web-search').disabled=teamo;if(teamo)$('#web-search').checked=false;$('#provider-help').textContent=teamo?'接口：api.teamorouter.com · 模型 ID 无厂商前缀。支持对话、工具与主动评估；此接入不提供 OpenRouter 联网插件。':'接口：openrouter.ai · 模型 ID 包含厂商前缀。两家服务商的 Key 分别保存。';$('#settings-status').textContent='';}
$('#provider').addEventListener('change',loadProvider);
$('#settings-form').addEventListener('submit',e=>{e.preventDefault();saveSettings();});$('#save-test').addEventListener('click',()=>saveSettings(true));
$('#load-models').addEventListener('click',e=>action(e.currentTarget,async()=>{if($('#provider').value!==data.settings.provider || $('#api-key').value.trim())throw new Error('请先保存当前服务商与 Key，再加载模型列表。');const response=await api('models');$('#models').replaceChildren(...response.models.map(m=>{const option=document.createElement('option');option.value=m.id;option.label=m.name;return option;}));toast(`已加载 ${response.models.length} 个模型，输入模型名称可筛选。`);}));
$('#remove-key').addEventListener('click',e=>action(e.currentTarget,async()=>{if(!confirm('移除当前服务商的 Key？新的模型调用将停止。'))return;await post('settings',{...collectSettings(),clear_key:true,api_key:null});settingsLoaded=false;await refresh();loadSettings();toast('Key 已移除');}));
async function send(text){if(sending)return;sending=true;$('#send').disabled=true;try{await post('chat',{text,request_id:crypto.randomUUID()});$('#chat-input').value='';await refresh();window.scrollTo({top:document.documentElement.scrollHeight,behavior:'auto'});}catch(e){toast(e.message,true);}finally{sending=false;render();}}
$('#chat-form').addEventListener('submit',e=>{e.preventDefault();const text=$('#chat-input').value.trim();if(text)send(text);});
$('#chat-input').addEventListener('keydown',e=>{if(e.key==='Enter'&&!e.shiftKey&&!e.isComposing){e.preventDefault();if(!$('#send').disabled)$('#chat-form').requestSubmit();}});
$('#stop-chat').addEventListener('click',e=>action(e.currentTarget,async()=>{await post('chat/cancel');await refresh();toast('已停止生成，可以继续下一条消息。');}));
$('#messages').addEventListener('click',e=>{const suggestion=e.target.closest('[data-suggestion]');if(suggestion){$('#chat-input').value=suggestion.dataset.suggestion;$('#chat-input').focus();}const retry=e.target.closest('[data-retry-message]');if(retry){const m=data.messages.find(m=>m.id===retry.dataset.retryMessage);if(m)send(m.content);}});
$('#memory-form').addEventListener('submit',e=>{e.preventDefault();action($('#memory-form button[type=submit]'),async()=>{const result=await post('memories',{content:$('#memory-input').value.trim()});$('#memory-input').value='';await refresh();toast(result.retained==null?'记忆已保存':`Jev 已整理，保留 ${result.retained} 条记忆。`);});});
$('#memories-list').addEventListener('click',e=>{const button=e.target.closest('[data-forget]');if(button)action(button,async()=>{if(!confirm('删除这条记忆及依赖它生成的反思、回复与任务成果？进行中的相关生成也会失效。'))return;const result=await api(`memories/${button.dataset.forget}`,{method:'DELETE'});await refresh();toast(`已清除 ${result.removed} 条记忆及关联记录`);});});
$('#reflect').addEventListener('click',e=>action(e.currentTarget,async()=>{if(!data.messages.length&&!data.memories.length){toast('先留下一些对话或记忆，再整理反思。');return;}await post('jobs',{kind:'reflection',objective:'整理最近的对话与已保存记忆，记录有依据的理解和待验证问题。',delay_minutes:0});await refresh();toast('反思任务已创建，Jev 将判断是否保留成果。');}));
$('#refresh').addEventListener('click',()=>refresh());
navigate(location.hash.slice(1)||'chat');refresh();setInterval(refresh,2500);document.addEventListener('visibilitychange',()=>{if(!document.hidden)refresh();});

let feishuLoaded=false;
async function refreshFeishu(){try{const f=await api('feishu');const c=f.config;const labels={disconnected:'未连接',connecting:'正在连接',connected:'已连接',error:'连接需要处理'};$('#feishu-status').textContent=`${labels[f.status.state]||f.status.state} · 已发送 ${f.delivery.sent} 条${f.delivery.failed?' · '+f.delivery.failed+' 条发送状态需核对':''}${f.status.error?' · '+f.status.error:''}`;if(!feishuLoaded){$('#feishu-profile').value=c.profile;$('#feishu-mode').value=c.mode||'poll';feishuLoaded=true;}$('#feishu-connect').hidden=c.enabled;$('#feishu-disconnect').hidden=!c.enabled;$('#feishu-profile').disabled=c.enabled;$('#feishu-mode').disabled=c.enabled;$('#feishu-open').hidden=!c.chat_id;if(c.chat_id)$('#feishu-open').href=`https://applink.feishu.cn/client/chat/open?openChatId=${encodeURIComponent(c.chat_id)}`;}catch(e){$('#feishu-status').textContent=e.message;}}
$('#feishu-connect').addEventListener('click',e=>action(e.currentTarget,async()=>{await post('feishu/connect',{profile:$('#feishu-profile').value.trim(),mode:$('#feishu-mode').value});feishuLoaded=false;await refreshFeishu();toast('接入说明已发送到你的飞书。');}));
$('#feishu-disconnect').addEventListener('click',e=>action(e.currentTarget,async()=>{await post('feishu/disconnect');await refreshFeishu();toast('飞书已断开，本机服务继续运行。');}));
refreshFeishu();setInterval(()=>{if(view==='settings')refreshFeishu();},5000);

let bootstrapShown=false;
async function refreshBootstrap(){try{const b=await api('bootstrap');$('#bootstrap-runs').innerHTML=b.runs.map(r=>`<div class="bootstrap-run"><strong>${states[r.status]}</strong> · ${r.processed}/${r.total} 批<p>${escape(r.output||r.report||'等待 Jev、外部调用权限与调用额度就绪。')}</p>${r.error?`<p class="item-error">${escape(r.error)}</p>`:''}${!r.revoked&&r.status==='failed'?`<button type="button" class="text-button" data-retry-bootstrap="${escape(r.id)}">继续处理</button>`:''}${!r.revoked&&['queued','running','failed'].includes(r.status)?`<button type="button" class="text-button danger" data-revoke-bootstrap="${escape(r.id)}">撤销本次授权</button>`:''}</div>`).join('');if(b.needs_prompt&&!bootstrapShown){bootstrapShown=true;$('#bootstrap-dialog').showModal();}}catch(e){$('#bootstrap-runs').textContent=e.message;}}
$('#bootstrap-open').addEventListener('click',()=>{$('#bootstrap-error').textContent='';$('#bootstrap-dialog').showModal();});
async function dismissBootstrap(){await post('bootstrap/dismiss');$('#bootstrap-dialog').close();}
$('#bootstrap-later').addEventListener('click',e=>action(e.currentTarget,dismissBootstrap));
$('#bootstrap-dialog').addEventListener('cancel',e=>{e.preventDefault();action(null,dismissBootstrap);});
$('#bootstrap-form').addEventListener('submit',e=>{e.preventDefault();action(e.submitter,async()=>{try{await post('bootstrap/authorize',{files_allowed:$('#bootstrap-files').checked,paths:$('#bootstrap-paths').value.split('\n').map(s=>s.trim()).filter(Boolean),feishu_allowed:$('#bootstrap-feishu').checked,history_days:Number($('#bootstrap-days').value),cloud_processing_allowed:$('#bootstrap-cloud').checked});$('#bootstrap-dialog').close();$('#bootstrap-form').reset();await refresh();await refreshBootstrap();navigate('settings');toast('冷启动任务已创建，完成后会自动更新上下文。');}catch(err){$('#bootstrap-error').textContent=err.message;throw err;}});});
$('#bootstrap-runs').addEventListener('click',e=>{const button=e.target.closest('[data-revoke-bootstrap]');if(button)action(button,async()=>{await post('bootstrap/revoke',{job_id:button.dataset.revokeBootstrap});await refreshBootstrap();await refresh();toast('已撤销授权，未完成的暂存资料已清除。');});});
refreshBootstrap();setInterval(refreshBootstrap,5000);

$('#memories-list').addEventListener('click',e=>{const b=e.target.closest('[data-restore-profile]');if(b)action(b,async()=>{await post(`profiles/${b.dataset.restoreProfile}/restore`);await refresh();toast('已恢复偏好；原有版本仍可查看。');});});

let heartbeatLoaded=false;
async function refreshHeartbeat(){try{const h=await api('heartbeat'),c=h.config;$('#decision-tendencies').textContent='当前分支倾向（非概率）：'+Object.entries(h.branch_tendencies||{}).map(([k,v])=>({message:'主动联系',continue_interest:'延续兴趣',explore:'自由探索',continue_work:'推进现有工作（旧）',new_work:'探索新工作（旧）',no_action:'暂不行动'}[k]||k)+' '+Number(v).toFixed(0)).join(' · ');$('#agent-loop-phase').textContent='主循环阶段：'+({github_weekly:'更新 GitHub 周榜',hot_topics:'更新每小时热搜',news:'更新每日新闻',import:'资料导入',continuations:'推进未完成工作',decision:'Jev 判定与执行',idle:'等待下一轮'}[h.loop?.phase]||'等待')+' · 用户对话独立即时响应';if(!heartbeatLoaded){$('#heartbeat-enabled').checked=c.enabled;$('#heartbeat-interval').value=c.interval_minutes;$('#heartbeat-topics').value=c.public_topics.join('\n');heartbeatLoaded=true;}$('#heartbeat-status').textContent=c.enabled?`已开启 · ${h.next_at>Date.now()?'下次唤醒 '+time(h.next_at):'等待运行条件就绪'} · 行动与联系节奏由 Jev 判断`:'自主循环已关闭';$('#heartbeat-runs').innerHTML=h.runs.slice(0,8).map(r=>`<div class="bootstrap-run"><span class="field-help">${time(r.created_at)}</span> · ${{message:'主动联系',continue_interest:'延续兴趣',explore:'自由探索',continue_work:'推进现有工作（旧）',new_work:'探索新工作（旧）',no_action:'暂不行动',organize:'整理自己（旧记录）',wait:'保持安静（旧记录）'}[r.action]||'正在判定'}${r.tool?' · '+escape(r.tool):''}${r.intention?'<p class="field-help">本轮用意：'+escape(r.intention)+'</p>':''}<p class="field-help">${r.job_status?'内部处理：'+states[r.job_status]:r.status==='done'?'本次判断完成':r.status==='running'?'正在处理':r.status==='interrupted'?'重启后等待下个周期':'本次未完成'}${r.error?' · '+escape(r.error):''}</p></div>`).join('');}catch(e){$('#heartbeat-status').textContent=e.message;}}
$('#heartbeat-form').addEventListener('submit',e=>{e.preventDefault();action(e.submitter,async()=>{await post('heartbeat',{enabled:$('#heartbeat-enabled').checked,interval_minutes:Number($('#heartbeat-interval').value),public_topics:$('#heartbeat-topics').value.split('\n').map(s=>s.trim()).filter(Boolean)});heartbeatLoaded=false;await refreshHeartbeat();await refresh();toast('自主循环设置已保存');});});
$('#heartbeat-wake').addEventListener('click',e=>action(e.currentTarget,async()=>{await post('heartbeat/wake');await refreshHeartbeat();toast('已安排唤醒，Jev 将在空闲时判断下一步。');}));
refreshHeartbeat();setInterval(refreshHeartbeat,5000);

// Compact trajectory ledger: metadata stays light; full bodies load on expansion.
const tracePurposes={jev_tree_feedback:'Jev 分支倾向更新',jev_tree_direction:'Jev 本轮方向',jev_tree_intention:'Jev 具体用意',jev_dialogue_context:"Jev 对话上下文筛选",goal_proposal:'三层自然语言整理',jev_goal:'Jev 叙述与关注调整',tool_proposal:'工具参数提案',jev_tool_execution:'Jev 工具执行判定',jev_daily_news:'Jev 每日新闻精选',self_organization:'主循环整理',autonomous_message:'主动对话',jev_heartbeat:'定时唤醒判定',jev_heartbeat_tool:'整理工具选择',jev_memory:'记忆筛选',jev_task:'任务判定',jev_adaptation:'人格与用户理解更新',jev_plan:'下一步判定',reflection:'内部反思',dialogue:'对话',work:'后台工作',appraisal:'主动评估',notification:'主动通知',connection_test:'连接测试'};
let traceFilter='all',traceSignature='';
const traceLoads=new Set();
function renderTraces(){
  if(!data)return;
  const search=$('#trace-search').value.trim().toLowerCase();
  const calls=data.calls.filter(c=>(traceFilter==='all'||(traceFilter==='jev'?c.model==='jev':traceFilter==='chat'?c.model!=='jev':['failed','unknown'].includes(c.status)))&&`${c.model} ${c.purpose} ${tracePurposes[c.purpose]||''} ${c.error||''}`.toLowerCase().includes(search));
  $('#trace-count').textContent=`${calls.length} / ${data.calls.length} 条`;
  const signature=JSON.stringify(calls);
  if(signature!==traceSignature){
    traceSignature=signature;
    const list=$('#calls-list'),old=new Map([...list.children].map(el=>[el.dataset.call,el]));
    if(!calls.length){list.innerHTML='<div class="empty-state">没有匹配的模型交互。</div>';return;}
    const active=new Set(calls.map(c=>c.id));
    for(const child of [...list.children])if(!active.has(child.dataset.call))child.remove();
    calls.forEach((c,i)=>{
      let row=old.get(c.id);
      if(!row){row=document.createElement('details');row.className='trace-call';row.dataset.call=c.id;row.innerHTML='<summary></summary><div class="trace-body"><p class="field-help">正在读取交互详情…</p></div>';row.addEventListener('toggle',()=>{if(row.open)loadTrace(row);});}
      row.querySelector('summary').innerHTML=`<span class="trace-index">${String(data.calls.length-data.calls.indexOf(c)).padStart(2,'0')}</span><time>${time(c.created_at)}</time><span class="trace-model">${escape(c.model)}</span><span class="trace-purpose">${escape(tracePurposes[c.purpose]||c.purpose)}</span><span class="trace-status ${['failed','unknown'].includes(c.status)?'trace-error':''}">${escape(states[c.status]||c.status)}</span><span class="trace-tokens">${c.tokens??'—'} <small>Token</small></span>`;
      if(list.children[i]!==row)list.insertBefore(row,list.children[i]||null);
    });
  }
  for(const row of $$('#calls-list > details[open]'))loadTrace(row);
}
async function loadTrace(row){
  const id=row.dataset.call;if(traceLoads.has(id))return;traceLoads.add(id);
  try{
    const trace=await api(`calls/${encodeURIComponent(id)}`);if(!row.isConnected)return;
    const c=data.calls.find(c=>c.id===id);if(!c)return;
    const signature=JSON.stringify([trace,c.status,c.error]);if(row._traceSignature===signature)return;row._traceSignature=signature;
    const body=row.querySelector('.trace-body');const opened=new Set([...body.querySelectorAll('details[open]')].map(d=>d.dataset.panel));
    const raw=(label,value,key,open=false)=>`<details class="trace-panel" data-panel="${key}" ${opened.has(key)||(!row._loaded&&open)?'open':''}><summary>${label}</summary><pre>${escape(typeof value==='string'?value:JSON.stringify(value,null,2))}</pre></details>`;
    if(!trace.available){body.innerHTML=`${c.error?`<p class="trace-error">${escape(c.error)}</p>`:''}<p class="field-help">这条旧记录未保存完整报文，或报文已随记忆删除而清除。新版调用会记录输入、输出与最终选择。</p>`;return;}
    let html=`<div class="trace-meta"><code>${escape(trace.endpoint)}</code><span>${trace.http_status?'HTTP '+trace.http_status:'尚无响应'} · ${trace.finished_at?((trace.finished_at-c.created_at)/1000).toFixed(2)+' s':c.status==='running'?'进行中':'已中断'} · ${money(c.cost)}</span></div>`;
    if(c.error)html+=`<p class="trace-error">${escape(c.error)}</p>`;
    if(c.model==='jev'){
      html+='<p class="field-help">概率按接口原值展示。自主方向和用意按正概率抽样；其他判定采用最高概率选项。</p>';
      for(const [name,q] of Object.entries(trace.request?.questions||{})){
        const answer=trace.response?.answers?.[name]||{},selection=trace.selections?.[name];
        html+=`<section class="trace-question"><div class="trace-question-title"><strong>${escape(name)}</strong><span class="status-tag">${selection?.selected?'最终选择 · '+escape(selection.selected):q.type==='noul'?'Noul · '+escape(answer.noul??'—'):selection?.error?'选择失败':'尚未采用选项'}</span></div><p>${escape(q.instructions||'')}</p>`;
        const options=Object.entries(q.criteria||{}).sort(([a],[b])=>(Number(answer.probabilities?.[b])||0)-(Number(answer.probabilities?.[a])||0));
        for(const [key,description] of options){const probability=answer.probabilities?.[key],valid=typeof probability==='number'&&Number.isFinite(probability);html+=`<div class="trace-option ${selection?.selected===key?'chosen':''}"><div class="trace-option-label"><strong>${escape(key)}</strong><span>${valid?escape((probability*100).toFixed(2))+'%':'—'}${selection?.selected===key?' · 已选择':''}</span></div><div class="trace-bar"><i style="width:${valid?Math.max(0,Math.min(100,probability*100)):0}%"></i></div><p>${escape(typeof description==='string'?description:JSON.stringify(description))}</p></div>`;}
        if(answer.choice&&answer.choice!==selection?.selected)html+=`<p class="field-help">接口返回 choice：${escape(answer.choice)}；执行以记录的最终选择为准。</p>`;
        if(selection?.method)html+=raw('选择方式与随机种子',selection,'sampling-'+name);
        if(selection?.error)html+=`<p class="trace-error">${escape(selection.error)}</p>`;
        html+='</section>';
      }
    }else{
      const message=trace.response?.choices?.[0]?.message;
      if(message?.content)html+=raw('模型输出',message.content,'content',true);
      if(message?.tool_calls)html+=raw('工具调用',message.tool_calls,'tools',true);
    }
    const usage=trace.response?.usage;
    if(usage)html+=`<p class="field-help">输入 ${escape(usage.prompt_tokens??usage.input_tokens??'—')} · 输出 ${escape(usage.completion_tokens??usage.output_tokens??'—')} Token</p>`;
    html+=raw(c.model==='jev'?'完整输入 · 上下文与问题':'完整输入 · 消息与参数',trace.request,'request');
    html+=raw('原始响应',trace.response??'尚未收到完整响应。','response');
    body.innerHTML=html;row._loaded=true;
  }catch(e){if(row.isConnected)row.querySelector('.trace-body').textContent=e.message;}finally{traceLoads.delete(id);}
}
$('.trace-toolbar').addEventListener('click',e=>{const button=e.target.closest('[data-trace-filter]');if(!button)return;traceFilter=button.dataset.traceFilter;for(const b of $$('[data-trace-filter]'))b.setAttribute('aria-pressed',String(b===button));renderTraces();});
$('#trace-search').addEventListener('input',renderTraces);

$('#bootstrap-runs').addEventListener('click',e=>{const button=e.target.closest('[data-retry-bootstrap]');if(button)action(button,async()=>{await post(`jobs/${button.dataset.retryBootstrap}/retry`);await refreshBootstrap();toast('已安排继续处理。');});});

async function refreshWeixin(){try{const w=await api('weixin');const labels={connected:'已连接',connecting:'正在连接',error:'连接需要处理',disconnected:'未连接'};$('#weixin-status').textContent=(labels[w.status?.state]||'未连接')+(w.enabled&&!w.can_send?' · 请先在微信发送一条消息':'')+(w.status?.error?' · '+w.status.error:'')+(w.delivery_uncertain?' · '+w.delivery_uncertain+' 条发送状态需核对':'');$('#weixin-disconnect').hidden=!w.enabled;$('#weixin-login').hidden=w.enabled;$('#weixin-login-status').textContent=({wait:'请用本人微信扫码',scaned:'已扫码，请在微信确认',confirmed:'微信连接成功',expired:'二维码已过期，请重新扫码'})[w.login_status]||'';if(w.enabled||w.login_status==='expired')$('#weixin-qr').hidden=true;}catch(e){$('#weixin-status').textContent=e.message;}}
$('#weixin-login').addEventListener('click',e=>action(e.currentTarget,async()=>{const w=await post('weixin/login');$('#weixin-qr').src=w.image;$('#weixin-qr').hidden=false;await refreshWeixin();}));
$('#weixin-disconnect').addEventListener('click',e=>action(e.currentTarget,async()=>{await post('weixin/disconnect');$('#weixin-qr').hidden=true;await refreshWeixin();}));
refreshWeixin();setInterval(()=>{if(view==='settings')refreshWeixin();},4000);

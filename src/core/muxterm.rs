//! Muxterm product composition root.
//!
//! `Muxterm` owns the cross-domain state needed by one product session.  The
//! C ABI keeps a raw pointer to this value for compatibility, but the handle
//! is no longer the owner of a single runtime: it owns Catalog, Projects,
//! WorkspacePool and Activity-facing attention state together.

use std::collections::VecDeque;
use std::ffi::{c_char, CString};
use std::ptr;
use std::sync::Arc;

use crate::activity::attention::signal::AttentionSignal;
use crate::activity::{ActivityContext, ActivityState};
use crate::catalog::{ResolveError, ResolvedTarget};
use crate::config::SettingsService;
use crate::projects::ProjectsService;
use crate::protocol::candidate::{OpenRequest, ResolveIntent};
use crate::protocol::state::{PaneAgentInfo, StateChange};
use crate::runtime::registry::RuntimeRegistry;
use crate::runtime::{
    runtime_supports_channels, ControlEvent, RenderEvent, Runtime, RuntimeBatch, RuntimeSignal,
};
use crate::workspace::pool::WorkspacePool;
use crate::workspace::spec::WorkspaceSpec;
use crate::workspace::template::TemplateRegistry;
use crate::workspace::workspace::Workspace;

use crate::activity::record::ActivityEvent;
use crate::protocol::ffi::callbacks::FfiCallbacks;
use crate::protocol::ffi::types::CLayoutNode;
use crate::transport::registry::{ConnectionRegistry, TransportRegistry};

type PendingAttentionUpdate = (u32, Vec<AttentionSignal>, String, u64, Option<String>);

#[derive(Debug, Clone, Copy)]
enum CommandActivityPhase {
    Start,
    Done(Option<u8>),
}

type PendingCommandActivity = (u32, Option<String>, CommandActivityPhase);

/// One product session composed from Core domains.
///
/// The fields remain crate-visible while the FFI function modules are being
/// migrated to service methods.  Frontends never receive this Rust object;
/// they use the public FFI facade and its safe client wrapper.
pub struct Muxterm {
    /// Catalog: provider/discovery/resolution services.
    pub(crate) catalog: crate::catalog::Catalog,
    /// Runtime provider registry owned by this product session.
    pub(crate) runtime_registry: Arc<RuntimeRegistry>,
    /// Transport provider registry owned by this product session.
    pub(crate) transport_registry: Arc<TransportRegistry>,
    /// Reusable target connections owned by the product session.
    pub(crate) connections: ConnectionRegistry,
    /// Create-time workspace templates projected from Config.
    pub(crate) templates: TemplateRegistry,
    /// The single live WorkspacePool owned by the product session.
    ///
    /// The product root owns the live runtime instances and the reusable
    /// target connections used to construct them.
    pub(crate) pool: WorkspacePool,
    /// Core-owned Project records projected from the same SettingsService.
    pub(crate) projects: ProjectsService,
    /// Synchronous executor used by the C ABI boundary.
    pub(crate) rt: tokio::runtime::Runtime,
    pub(crate) callbacks: FfiCallbacks,
    /// Cross-workspace attention aggregation; Activity will own this later.
    pub(crate) activity: ActivityState,
    /// Core-owned configuration service shared by adapters.
    pub(crate) settings: SettingsService,
    /// C buffers whose pointers remain valid until the next poll/query.
    pub(crate) event_data: Vec<Vec<u8>>,
    pub(crate) event_names: Vec<CString>,
    pub(crate) tab_names: Vec<CString>,
    pub(crate) layout_nodes: Vec<CLayoutNode>,
    /// Lane-separated batches waiting for the legacy C ABI conversion.
    pub(crate) deferred_batches: VecDeque<(crate::protocol::WorkspaceId, RuntimeBatch)>,
    /// Product Activity lane events waiting for the Activity FFI poll.
    pub(crate) deferred_activity_events: VecDeque<(crate::protocol::WorkspaceId, ActivityEvent)>,
    pub(crate) workspace_ids: Vec<CString>,
}

impl Muxterm {
    /// Construct a Runtime through the product composition root.
    ///
    /// Catalog provides only the registered provider view; this entry point
    /// owns the provider lookup and connection-backed Runtime construction.
    pub fn new_runtime(
        catalog: &crate::catalog::Catalog,
        connections: &mut ConnectionRegistry,
        spec: &WorkspaceSpec,
    ) -> anyhow::Result<Box<dyn Runtime>> {
        let runtime_registry = catalog.runtime_registry();
        let transport_registry = catalog.transport_registry();
        Self::new_runtime_parts(
            runtime_registry.as_ref(),
            transport_registry.as_ref(),
            connections,
            spec,
        )
    }

    pub(crate) fn pool(&self) -> &WorkspacePool {
        &self.pool
    }

    pub(crate) fn pool_mut(&mut self) -> &mut WorkspacePool {
        &mut self.pool
    }

    pub(crate) fn projects(&self) -> &ProjectsService {
        &self.projects
    }

    pub(crate) fn projects_mut(&mut self) -> &mut ProjectsService {
        &mut self.projects
    }

    /// Resolve and open a product workspace through the single live pool.
    ///
    /// The provider lookup stays in Catalog, while pool insertion and
    /// template application belong to the composition root that owns the
    /// runtime instances.
    pub(crate) async fn open_spec(
        &mut self,
        spec: &WorkspaceSpec,
    ) -> anyhow::Result<&mut Workspace> {
        Self::open_spec_parts(
            &self.runtime_registry,
            &self.transport_registry,
            &mut self.connections,
            &self.templates,
            &mut self.pool,
            spec,
        )
        .await
    }

    /// Open using explicitly split owner fields. The FFI boundary uses this
    /// form so its Tokio runtime can be borrowed independently from Catalog
    /// and the live pool.
    pub(crate) async fn open_spec_parts<'a>(
        runtime_registry: &RuntimeRegistry,
        transport_registry: &TransportRegistry,
        connections: &'a mut ConnectionRegistry,
        templates: &TemplateRegistry,
        pool: &'a mut WorkspacePool,
        spec: &WorkspaceSpec,
    ) -> anyhow::Result<&'a mut Workspace> {
        let workspace_id = spec.id();
        let should_apply_template = pool.get(&workspace_id).is_none() && spec.create;
        let template = spec
            .template
            .as_ref()
            .and_then(|name| templates.get(name))
            .cloned();
        let runtime =
            Self::new_runtime_parts(runtime_registry, transport_registry, connections, spec)?;
        let workspace = pool
            .open_spec_with_runtime(spec, runtime)
            .await
            .map_err(|error| runtime_open_error(spec, error))?;
        if should_apply_template {
            if let Some(template) = template {
                workspace.start_template(template)?;
            }
        }
        Ok(workspace)
    }

    /// Construct a Runtime from Catalog's provider view. The live Runtime is
    /// owned by the product pool, while the reusable target connection stays
    /// owned by this composition root.
    pub(crate) fn new_runtime_parts(
        runtime_registry: &RuntimeRegistry,
        transport_registry: &TransportRegistry,
        connections: &mut ConnectionRegistry,
        spec: &WorkspaceSpec,
    ) -> anyhow::Result<Box<dyn Runtime>> {
        let runtime_id = spec.runtime.as_str();
        let transport_id = spec.transport.as_str();
        let provider =
            runtime_registry
                .get(runtime_id)
                .ok_or_else(|| ResolveError::UnknownRuntime {
                    id: runtime_id.to_string(),
                })?;
        let transport =
            transport_registry
                .get(transport_id)
                .ok_or_else(|| ResolveError::UnknownTransport {
                    id: transport_id.to_string(),
                })?;
        let required = provider.channel_requirements().to_vec();
        let supported = transport.supported_channels().to_vec();
        if !runtime_supports_channels(provider, &supported) {
            return Err(ResolveError::IncompatibleChannels {
                runtime_id: runtime_id.to_string(),
                transport_id: transport_id.to_string(),
                required,
                supported,
            }
            .into());
        }
        let target = spec.alias.as_deref().unwrap_or("");
        let connection = if let Some(existing) = connections.get(transport_id, target) {
            existing
        } else {
            connections
                .acquire(transport_id, target, || transport.connect(target))
                .map_err(|error| ResolveError::TargetConnection {
                    transport_id: transport_id.to_string(),
                    target: target.to_string(),
                    message: format!("{error:#}"),
                })?
        };
        provider
            .new_instance(Arc::clone(&connection), &spec.runtime_spec())
            .map_err(|error| ResolveError::RuntimeOpen {
                runtime_id: runtime_id.to_string(),
                transport_id: transport_id.to_string(),
                target: target.to_string(),
                message: format!("{error:#}"),
            })
            .map_err(Into::into)
    }

    /// Open a resolver result and retain its canonical descriptor in Core.
    pub(crate) async fn open_resolved(
        &mut self,
        resolved: ResolvedTarget,
    ) -> anyhow::Result<&mut Workspace> {
        Self::open_resolved_parts(
            &self.runtime_registry,
            &self.transport_registry,
            &mut self.connections,
            &self.templates,
            &mut self.pool,
            resolved,
        )
        .await
    }

    /// Open a resolved target using explicitly split owner fields.
    pub(crate) async fn open_resolved_parts<'a>(
        runtime_registry: &RuntimeRegistry,
        transport_registry: &TransportRegistry,
        connections: &'a mut ConnectionRegistry,
        templates: &TemplateRegistry,
        pool: &'a mut WorkspacePool,
        resolved: ResolvedTarget,
    ) -> anyhow::Result<&'a mut Workspace> {
        let workspace_id = resolved.workspace_id();
        if let Some(existing) = pool.get(&workspace_id) {
            if existing.resolved_target().map(|r| &r.spec) == Some(&resolved.spec) {
                return Ok(pool.get_mut(&workspace_id).expect("刚查过必须存在"));
            }
            anyhow::bail!(
                "identity key 撞到已打开 WorkspaceId {}（spec 不一致）",
                workspace_id
            );
        }
        let spec = resolved.spec.clone();
        let canonical = resolved.canonical.clone();
        let workspace = Self::open_spec_parts(
            runtime_registry,
            transport_registry,
            connections,
            templates,
            pool,
            &spec,
        )
        .await?;
        workspace.set_resolved_target(ResolvedTarget { canonical, spec });
        Ok(workspace)
    }

    /// Create a native Runtime worktree and open the resulting Workspace in
    /// the product-owned pool. This keeps live-workspace orchestration on the
    /// composition root's product path.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_native_worktree_with_pool(
        runtime_registry: &RuntimeRegistry,
        transport_registry: &TransportRegistry,
        connections: &mut ConnectionRegistry,
        templates: &TemplateRegistry,
        pool: &mut WorkspacePool,
        source: &crate::protocol::WorkspaceId,
        worktree: &crate::runtime::WorktreeCreateSpec,
        provenance: Option<crate::workspace::provenance::WorkspaceProvenance>,
        template: Option<crate::workspace::template::TemplateName>,
    ) -> anyhow::Result<crate::protocol::WorkspaceId> {
        let mut spec = {
            let workspace = pool
                .get(source)
                .ok_or_else(|| anyhow::anyhow!("workspace {source} 不在池里"))?;
            if !workspace
                .runtime()
                .support()
                .contains(&crate::runtime::RuntimeCapability::WorktreeCreate)
            {
                anyhow::bail!("runtime 不支持 native WorktreeCreate");
            }
            WorkspaceSpec::from_runtime_spec(workspace.runtime().create_worktree_spec(worktree)?)
        };
        spec.provenance = provenance.clone();
        spec.template = template;
        let workspace_id = spec.id();
        let should_apply_template = pool.get(&workspace_id).is_none() && spec.create;
        let template_record = spec
            .template
            .as_ref()
            .and_then(|name| templates.get(name))
            .cloned();
        let runtime =
            Self::new_runtime_parts(runtime_registry, transport_registry, connections, &spec)?;
        let workspace = pool.open_spec_with_runtime(&spec, runtime).await?;
        workspace.set_provenance(provenance);
        if should_apply_template {
            if let Some(template) = template_record {
                workspace.start_template(template)?;
            }
        }
        Ok(workspace_id)
    }

    /// Resolve a product open request using recent descriptors from the live
    /// pool, without making Catalog own or borrow that pool.
    pub(crate) fn resolve_open_request(
        &mut self,
        request: &OpenRequest,
    ) -> Result<ResolvedTarget, crate::catalog::ResolveError> {
        let recent: Vec<ResolvedTarget> = self
            .pool
            .list()
            .into_iter()
            .filter_map(|workspace| workspace.resolved_target().cloned())
            .collect();
        self.catalog.resolve_open_request_with_recent(
            &mut self.connections,
            request,
            self.projects.list_projects(),
            &recent,
        )
    }

    /// Resolve and open a compatibility TargetConfig without exposing a spec
    /// to the frontend caller.
    pub(crate) async fn open_target(
        &mut self,
        config: &crate::projects::TargetConfig,
        intent: ResolveIntent,
    ) -> anyhow::Result<&mut Workspace> {
        let resolved = self
            .catalog
            .resolve_target(&mut self.connections, config, intent)?;
        self.open_resolved(resolved).await
    }

    pub(crate) fn active_workspace(&self) -> Option<&Workspace> {
        self.pool().active()
    }

    pub(crate) fn active_workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.pool_mut().active_mut()
    }

    pub(crate) fn clear_event_bufs(&mut self) {
        self.event_data.clear();
        self.event_names.clear();
    }

    pub(crate) fn push_name(&mut self, value: &str) -> *const c_char {
        match CString::new(value) {
            Ok(value) => {
                self.event_names.push(value);
                self.event_names.last().unwrap().as_ptr()
            }
            Err(_) => ptr::null(),
        }
    }

    pub(crate) fn push_data(&mut self, data: &[u8]) -> (*const u8, usize) {
        self.event_data.push(data.to_vec());
        let value = self.event_data.last().unwrap();
        (value.as_ptr(), value.len())
    }

    pub(crate) fn push_workspace_id(&mut self, value: &str) -> *const c_char {
        match CString::new(value) {
            Ok(value) => {
                self.workspace_ids.push(value);
                self.workspace_ids.last().unwrap().as_ptr()
            }
            Err(_) => ptr::null(),
        }
    }

    #[cfg(test)]
    pub(crate) fn defer_event(
        &mut self,
        workspace_id: crate::protocol::WorkspaceId,
        event: StateChange,
    ) {
        if should_export_state_change(&event) {
            self.defer_batch(workspace_id, RuntimeBatch::from(event));
        }
    }

    pub(crate) fn defer_batch(
        &mut self,
        workspace_id: crate::protocol::WorkspaceId,
        mut batch: RuntimeBatch,
    ) {
        // PaneIndexSnapshot is consumed by Core's Index and must never cross
        // the frontend Surface/FFI boundary. Keep the other lanes intact
        // until the C ABI adapter serializes one legacy event at a time.
        batch
            .render
            .retain(|event| !matches!(event, RenderEvent::PaneIndexSnapshot { .. }));
        if !batch.is_empty() {
            self.deferred_batches.push_back((workspace_id, batch));
        }
    }

    /// Convert only the batches selected for one legacy C ABI poll.
    ///
    /// `workspace_id = Some(id)` selects the active workspace for
    /// `muxterm_poll_events`; `None` drains all workspaces for the
    /// workspace-tagged poll. A partially consumed batch is requeued at the
    /// same position so `max_count` never drops the remaining lane events.
    pub(crate) fn take_deferred_events(
        &mut self,
        workspace_id: Option<&crate::protocol::WorkspaceId>,
        max_count: usize,
    ) -> Vec<(crate::protocol::WorkspaceId, StateChange)> {
        if max_count == 0 {
            return Vec::new();
        }

        let mut ready = Vec::new();
        let mut kept = VecDeque::new();
        for (id, batch) in self.deferred_batches.drain(..) {
            let selected = workspace_id.is_none_or(|wanted| wanted == &id);
            if !selected || ready.len() >= max_count {
                kept.push_back((id, batch));
                continue;
            }

            let events = batch.into_state_changes();
            let remaining = max_count - ready.len();
            let split = remaining.min(events.len());
            ready.extend(
                events[..split]
                    .iter()
                    .cloned()
                    .map(|event| (id.clone(), event)),
            );
            if split < events.len() {
                kept.push_back((
                    id,
                    RuntimeBatch::from_state_changes(events[split..].iter().cloned()),
                ));
            }
        }
        self.deferred_batches = kept;
        ready
    }

    /// Apply one lane-separated runtime batch to the cross-workspace Activity projection.
    pub(crate) fn apply_attention_for_batch(
        &mut self,
        ws_id: &crate::protocol::WorkspaceId,
        batch: &RuntimeBatch,
    ) {
        let mut pending: Vec<PendingAttentionUpdate> = Vec::new();
        let mut pending_process_names: Vec<(u32, Option<String>, bool)> = Vec::new();
        let mut pending_agents: Vec<(u32, Option<PaneAgentInfo>)> = Vec::new();
        let mut pending_commands: Vec<PendingCommandActivity> = Vec::new();
        let mut removed_panes = Vec::new();
        let mut attention_panes = Vec::new();
        {
            let Some(ws) = self.pool_mut().get_mut(ws_id) else {
                return;
            };
            for event in &batch.control {
                if let ControlEvent::PaneClosed { pane } = event {
                    removed_panes.push(pane.0);
                }
            }
            for event in &batch.signals {
                match event {
                    RuntimeSignal::PaneAgentChanged { pane, agent, .. } => {
                        pending_agents.push((pane.0, agent.as_deref().cloned()));
                        let process_name = agent.as_deref().and_then(|agent| {
                            [
                                agent.display_name.as_deref(),
                                agent.title.as_deref(),
                                agent.name.as_deref(),
                                agent.kind.as_deref(),
                            ]
                            .into_iter()
                            .flatten()
                            .find(|value| !value.trim().is_empty())
                            .map(str::to_string)
                        });
                        pending_process_names.push((pane.0, process_name, true));
                        attention_panes.push(*pane);
                    }
                    RuntimeSignal::StatusBarSubscription {
                        name,
                        value,
                        pane: Some(pane),
                    } if name.starts_with("muxterm.pane-cmd") => {
                        pending_process_names.push((
                            pane.0,
                            (!value.is_empty()).then(|| value.clone()),
                            false,
                        ));
                    }
                    RuntimeSignal::StatusBarSubscription { .. } => {}
                }
            }
            for event in batch
                .render
                .iter()
                .filter(|event| !matches!(event, RenderEvent::PaneOutput { .. }))
                .chain(
                    batch
                        .render
                        .iter()
                        .filter(|event| matches!(event, RenderEvent::PaneOutput { .. })),
                )
            {
                let pane = match event {
                    RenderEvent::PaneOutput { pane, .. }
                    | RenderEvent::PaneSnapshot { pane, .. }
                    | RenderEvent::PaneFrame { pane, .. }
                    | RenderEvent::PaneIndexSnapshot { pane, .. }
                    | RenderEvent::PaneHistory { pane, .. } => *pane,
                };
                attention_panes.push(pane);
            }
            for pane in attention_panes {
                let signals = ws.take_attention_signals(pane);
                let (last_line, seq) = ws.pane_last_line_seq(pane);
                let command_name = ws
                    .pane_command_marks(pane)
                    .last()
                    .map(|mark| mark.command.clone());
                let command_started = signals
                    .iter()
                    .any(|signal| matches!(signal, AttentionSignal::CommandStart));
                if command_started {
                    pending_commands.push((
                        pane.0,
                        command_name.clone(),
                        CommandActivityPhase::Start,
                    ));
                }
                for exit_code in signals.iter().filter_map(|signal| match signal {
                    AttentionSignal::CommandDone { exit_code } => Some(*exit_code),
                    _ => None,
                }) {
                    pending_commands.push((
                        pane.0,
                        command_name.clone(),
                        CommandActivityPhase::Done(exit_code),
                    ));
                }
                pending.push((
                    pane.0,
                    signals,
                    last_line,
                    seq,
                    command_started.then_some(command_name).flatten(),
                ));
            }
        }
        let ws_name = ws_id.replica_id();
        for (pane, agent) in pending_agents {
            if let Some(context) = self.activity_context(ws_id, pane) {
                let event = self.activity.apply_agent_signal(context, agent.as_ref());
                self.deferred_activity_events
                    .push_back((ws_id.clone(), event));
            }
        }
        for (pane, name, phase) in pending_commands {
            if let Some(context) = self.activity_context(ws_id, pane) {
                let event = match phase {
                    CommandActivityPhase::Start => self.activity.apply_command_start(context, name),
                    CommandActivityPhase::Done(exit_code) => {
                        self.activity.apply_command_done(context, name, exit_code)
                    }
                };
                self.deferred_activity_events
                    .push_back((ws_id.clone(), event));
            }
        }
        for (pane, name, is_agent) in pending_process_names {
            if is_agent {
                self.activity
                    .attention
                    .set_agent_process_name(&ws_name, pane, name);
            } else {
                self.activity
                    .attention
                    .set_process_name(&ws_name, pane, name);
            }
        }
        for (pane, signals, last_line, seq, command) in pending {
            if let Some(command) = command {
                self.activity
                    .attention
                    .set_process_name(&ws_name, pane, Some(command));
            }
            self.activity
                .attention
                .apply(&ws_name, pane, &signals, &last_line, seq);
        }
        for pane in removed_panes {
            self.activity.attention.remove_pane(&ws_name, pane);
            if let Some(event) = self
                .activity
                .remove_agent(ws_id, crate::protocol::PaneId(pane))
            {
                self.deferred_activity_events
                    .push_back((ws_id.clone(), event));
            }
        }
    }

    /// Compatibility adapter for callers that still provide the mixed event enum.
    #[cfg(test)]
    pub(crate) fn apply_attention_for_events(
        &mut self,
        ws_id: &crate::protocol::WorkspaceId,
        events: &[StateChange],
    ) {
        let batch = RuntimeBatch::from_state_changes(events.iter().cloned());
        self.apply_attention_for_batch(ws_id, &batch);
    }

    fn activity_context(
        &self,
        ws_id: &crate::protocol::WorkspaceId,
        pane: u32,
    ) -> Option<ActivityContext> {
        let workspace = self.pool().get(ws_id)?;
        let pane_id = crate::protocol::PaneId(pane);
        let tab = workspace
            .state()
            .pane(&pane_id)
            .map(|info| info.tab)
            .unwrap_or(crate::protocol::TabId(0));
        Some(ActivityContext {
            workspace: ws_id.clone(),
            pane: pane_id,
            tab,
            workspace_name: workspace.name().to_string(),
            runtime_name: workspace.state().workspace_runtime().to_string(),
            transport_name: ws_id.transport.clone(),
        })
    }

    pub(crate) fn take_activity_events(
        &mut self,
    ) -> Vec<(crate::protocol::WorkspaceId, ActivityEvent)> {
        self.deferred_activity_events.drain(..).collect()
    }
}

fn runtime_open_error(spec: &WorkspaceSpec, error: anyhow::Error) -> anyhow::Error {
    anyhow::Error::new(ResolveError::RuntimeOpen {
        runtime_id: spec.runtime.clone(),
        transport_id: spec.transport.clone(),
        target: spec.alias.clone().unwrap_or_default(),
        message: format!("{error:#}"),
    })
}

#[cfg(test)]
pub(crate) fn should_export_state_change(event: &StateChange) -> bool {
    !matches!(event, StateChange::PaneIndexSnapshot { .. })
}

use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use url::Url;

use miette::{IntoDiagnostic, Result, miette};
use notify_debouncer_full::{
    DebounceEventResult, Debouncer, NoCache, new_debouncer,
    notify::{self, EventKind, RecommendedWatcher, RecursiveMode, event::CreateKind},
};
use tokio::{runtime::Handle, sync::mpsc::Sender};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};
use tracing::{info, trace, warn};
use wax::Glob;

use crate::CONFIG;

#[derive(Debug, Clone)]
pub struct PathEvent {
    pub path: Arc<PathBuf>,
    pub kind: EventKind,
}

async fn create_debounced_watcher(
    path_event_tx: Sender<Arc<PathEvent>>,
) -> notify::Result<Debouncer<RecommendedWatcher, NoCache>> {
    let handle = Handle::current();

    let debouncer = new_debouncer(
        Duration::from_secs(CONFIG.debounce_sec),
        None,
        move |debounce_result: DebounceEventResult| {
            trace!("Debounce result: {:?}", debounce_result);
            let tx = path_event_tx.clone();
            let handle = handle.clone();
            handle.spawn(async move {
                match debounce_result {
                    Ok(events) => {
                        for event in events {
                            if event.event.kind.is_create()
                                || event.event.kind.is_modify()
                                || event.event.kind.is_remove()
                            {
                                info!("Accepted event: {:?}", event);
                                for path in event.paths.iter() {
                                    if let Err(e) = tx
                                        .send(Arc::new(PathEvent {
                                            path: Arc::new(path.clone()),
                                            kind: event.kind,
                                        }))
                                        .await
                                    {
                                        warn!("Error in debouncer send: {:?}", e);
                                    }
                                }
                            } else if event.kind.is_access() {
                                trace!("Skipping event: {:?}", event);
                            } else {
                                info!("Skipping event: {:?}", event);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Error in debouncer: {:?}", e);
                    }
                }
            });
        },
    )?;

    Ok(debouncer)
}

pub struct WatcherSubsystem {
    pub path_event_tx: Sender<Arc<PathEvent>>,
    pub first_path_scan: Arc<AtomicBool>,
}

#[async_trait]
impl IntoSubsystem<miette::Report> for WatcherSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        info!("Start path scanner");

        let url = Url::parse(&CONFIG.search.fuzzy.workspace_uri).into_diagnostic()?;

        if url.scheme() != "file" {
            return Err(miette!("Not a file URL: {}", url));
        }

        let path = url
            .to_file_path()
            .map_err(|_| miette!("Invalid file URL: {}", url))?;

        let positive = Glob::new(CONFIG.search.semantic.pattern.as_str()).into_diagnostic()?;

        info!("Start path scanner for {}", path.display());

        let walker = positive.walk(&path);

        for entry in walker
            .filter_map(|it| it.ok())
            .filter(|it| it.file_type().is_file())
        {
            info!("File found: {:?}", entry.path());
            self.path_event_tx
                .send(Arc::new(PathEvent {
                    path: Arc::new(entry.into_path()),
                    kind: EventKind::Create(CreateKind::File),
                }))
                .await
                .into_diagnostic()?;
        }
        info!("Path scanner finished, setting first path scan to true");

        self.first_path_scan.store(true, Ordering::Relaxed);

        info!("Start project files watcher for {}", path.display());

        let mut debouncer = create_debounced_watcher(self.path_event_tx.clone())
            .await
            .into_diagnostic()?;

        info!("Watching path: {:?}", path);

        debouncer
            .watch(path, RecursiveMode::Recursive)
            .into_diagnostic()?;

        info!("Project files watcher started");

        subsys.on_shutdown_requested().await;

        Ok(())
    }
}

#![allow(dead_code)]

use nix_utils::query_drv;

use crate::{
    db::Transaction,
    state::{BuildID, RemoteBuild},
};

#[tracing::instrument(skip(tx, res), err)]
pub async fn finish_build_step(
    tx: &mut Transaction<'_>,
    build_id: BuildID,
    step_nr: i32,
    res: &RemoteBuild,
    machine: Option<String>,
) -> anyhow::Result<()> {
    debug_assert!(res.start_time != 0);
    debug_assert!(res.stop_time != 0);
    tx.update_build_step_in_finish(crate::db::models::UpdateBuildStepInFinish {
        build_id,
        step_nr,
        status: res.step_status,
        error_msg: res.error_msg.as_deref(),
        start_time: i32::try_from(res.start_time)?,
        stop_time: i32::try_from(res.stop_time)?,
        machine: machine.as_deref(),
        overhead: if res.overhead != 0 {
            Some(res.overhead)
        } else {
            None
        },
        times_built: if res.times_build > 0 {
            Some(res.times_build)
        } else {
            None
        },
        is_non_deterministic: if res.times_build > 0 {
            Some(res.is_non_deterministic)
        } else {
            None
        },
    })
    .await?;
    debug_assert!(!res.log_file.is_empty());
    debug_assert!(!res.log_file.contains('\t'));

    tx.notify_step_finished(build_id, step_nr, &res.log_file)
        .await?;

    if res.step_status == crate::db::models::BuildStatus::Success {
        // Update the corresponding `BuildStepOutputs` row to add the output path
        let drv_path = tx.get_drv_path_from_build_step(build_id, step_nr).await?;
        if let Some(drv_path) = drv_path {
            // If we've finished building, all the paths should be known
            if let Some(drv) = query_drv(&drv_path).await? {
                for o in drv.outputs {
                    if let Some(path) = o.path {
                        tx.update_build_step_output(build_id, step_nr, &o.name, &path)
                            .await?;
                    }
                }
            }
        }
    }
    Ok(())
}

#[tracing::instrument(skip(db, o, build_id, build_opts, remote_store), err)]
pub async fn substitute_output(
    db: crate::db::Database,
    o: nix_utils::DerivationOutput,
    build_id: BuildID,
    drv_path: &str,
    build_opts: &nix_utils::BuildOptions,
    remote_store: Option<&nix_utils::RemoteStore<'_>>,
) -> anyhow::Result<()> {
    let Some(path) = &o.path else {
        return Ok(());
    };

    let starttime = i32::try_from(chrono::Utc::now().timestamp())?; // TODO
    nix_utils::realise_drv(path, build_opts).await?;
    if let Some(remote_store) = remote_store {
        remote_store.copy_path(path.to_owned()).await?;
    }
    let stoptime = i32::try_from(chrono::Utc::now().timestamp())?; // TODO

    let mut db = db.get().await?;
    let mut tx = db.begin_transaction().await?;
    tx.create_substitution_step(starttime, stoptime, build_id, drv_path, o)
        .await?;
    tx.commit().await?;

    Ok(())
}

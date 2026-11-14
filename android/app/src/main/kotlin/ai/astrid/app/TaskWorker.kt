package ai.astrid.app

import android.content.Context
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters

/**
 * The real half of the Scheduler screen: WorkManager drives this worker on
 * the schedule the user set, and the worker writes into the Rust core's
 * audit log through the bridge. The fired task then shows up in Live trace —
 * an actual Android-scheduled job producing a real receipt.
 */
class TaskWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val name = inputData.getString(KEY_TASK_NAME) ?: return Result.failure()
        return try {
            AstridBridge.init(applicationContext.filesDir)
            AstridBridge.bridge.appendEvent("schedule.fired", name)
            Result.success()
        } catch (t: Throwable) {
            Result.retry()
        }
    }

    companion object {
        const val KEY_TASK_NAME = "task_name"

        /** Unique work name for a task row (stable across re-enqueues). */
        fun workName(taskId: Long) = "astrid-task-$taskId"
    }
}

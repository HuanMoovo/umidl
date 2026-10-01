// umi-downloader 插件（内置示例）：失败重试钩子
// 能力面：umi.on / umi.retry / umi.storage / umi.export / umi.log
// 作用：监听 download:error，首次失败向宿主发起一次重试；
//       自己用 umi.storage 记住次数，重试后仍失败就放弃，绝不无限重试。
(function () {
  "use strict";

  var MAX_RETRY = 1;

  function taskIdOf(payload) {
    if (!payload) return "";
    return payload.taskId || payload.id || payload.task_id || "";
  }

  function onError(payload) {
    var taskId = taskIdOf(payload);
    if (!taskId) {
      umi.log("download:error 缺少 taskId，忽略");
      return;
    }
    var key = "retry:" + taskId;
    var used = umi.storage.get(key);
    if (typeof used !== "number") used = 0;
    if (used < MAX_RETRY) {
      umi.storage.set(key, used + 1);
      umi.retry(taskId, "第 " + (used + 1) + " 次失败自动重试");
      umi.log("任务 " + taskId + " 失败，已请求重试（" + (used + 1) + "/" + MAX_RETRY + "）");
    } else {
      umi.log("任务 " + taskId + " 已达重试上限 " + MAX_RETRY + "，不再重试");
    }
  }

  function onDone(payload) {
    var taskId = taskIdOf(payload);
    if (!taskId) return;
    umi.storage.set("retry:" + taskId, 0);
    umi.log("任务 " + taskId + " 成功，重试计数清零");
  }

  umi.on("download:error", onError);
  umi.on("download:done", onDone);

  umi.export("selfTest", function () {
    var probe = { taskId: "selftest-task" };
    onError(probe);
    if (umi.storage.get("retry:selftest-task") !== 1) {
      umi.log("selfTest 失败：首次失败应记 1 次重试");
      return false;
    }
    onError(probe);
    onError(probe);
    if (umi.storage.get("retry:selftest-task") !== 1) {
      umi.log("selfTest 失败：超过上限后不应继续重试");
      return false;
    }
    onDone(probe);
    if (umi.storage.get("retry:selftest-task") !== 0) {
      umi.log("selfTest 失败：成功后计数应清零");
      return false;
    }
    umi.log("retry-hook selfTest 通过");
    return true;
  });

  umi.log("retry-hook 就绪（umi " + umi.version + "）");
})();

# Resolve Blocking Tasks only when they are Done

Tasker will treat a Blocking Task as resolved only when it reaches Done. Canceled means the required dependency was abandoned, so automatically unblocking the parent would allow incomplete work to pass review or completion gates; agents must instead remove or replace the dependency, mark it non-blocking, rework the parent, or cancel the parent through the normal lifecycle.

Tasker will also exclude Blocked Tasks from normal agent pickup and reject transitions to Human Review, Integrating, or Done while any Blocking Task is unresolved, with Repair Override reserved for exceptional cleanup.

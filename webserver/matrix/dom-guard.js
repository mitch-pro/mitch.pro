// Keeps the chat client from crashing when something outside React (a
// translation feature, a managed-device extension, a content filter) moves or
// removes DOM nodes the client rendered. React then calls removeChild /
// insertBefore on a node that is no longer where it expects and the whole app
// shows "Unexpected Application Error ... The node to be removed is not a child
// of this node". These wrappers skip the call in that one case instead of
// throwing. Standard workaround: https://github.com/facebook/react/issues/11538
(function () {
  'use strict';
  if (!window.Node || Node.prototype.__mitchDomGuard) return;
  Node.prototype.__mitchDomGuard = true;

  var removeChild = Node.prototype.removeChild;
  Node.prototype.removeChild = function (child) {
    if (child && child.parentNode !== this) return child;
    return removeChild.apply(this, arguments);
  };

  var insertBefore = Node.prototype.insertBefore;
  Node.prototype.insertBefore = function (node, ref) {
    if (ref && ref.parentNode !== this) return node;
    return insertBefore.apply(this, arguments);
  };
})();

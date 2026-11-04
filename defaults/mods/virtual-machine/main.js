import { View, div } from "gpui-kit";
import * as herdr from "herdr";

// Supported native-vm builds host the VM view in this pane. Other clients
// can still open the shared layout and render an actionable fallback.
export default class VirtualMachine extends View {
  render() {
    const status = herdr.vmStatus?.() ?? { supported: false };
    return div().size_full().p_4().child(status.supported
      ? "Configure HERDR_VM_HOME and open this mod in a tab pane. See docs/local-vm.md."
      : "Local macOS VMs require an Apple Silicon build with --features native-vm. See docs/local-vm.md.");
  }
}

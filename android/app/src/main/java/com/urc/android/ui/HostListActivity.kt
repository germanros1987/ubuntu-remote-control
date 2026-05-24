package com.urc.android.ui

import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.text.method.LinkMovementMethod
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.Toast
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.core.text.HtmlCompat
import androidx.lifecycle.lifecycleScope
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.urc.android.MainActivity
import com.urc.android.R
import com.urc.android.databinding.ActivityHostListBinding
import com.urc.android.databinding.DialogAddHostBinding
import com.urc.android.databinding.ItemHostBinding
import com.urc.android.discovery.Host
import com.urc.android.discovery.HostStore
import com.urc.android.discovery.QrScanActivity
import com.urc.android.discovery.UrcUri
import com.urc.android.proxy.Cgnat
import com.urc.android.proxy.DEFAULT_WEB_TLS_PORT
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch

/**
 * Launcher screen: saved PCs + "Scan QR" + "Add manually". Tapping a host runs
 * the connect flow. Also handles the urc:// VIEW deep link: a tapped/scanned
 * pairing link lands here, gets saved, and connects straight through.
 */
class HostListActivity : AppCompatActivity() {

    private lateinit var binding: ActivityHostListBinding
    private lateinit var store: HostStore
    private val adapter = HostAdapter(
        onClick = { connect(it) },
        onLongClick = { confirmDelete(it) },
    )

    /** Latest known saved-host count, kept current by the hosts flow collector so
     *  onResume()'s onboarding check doesn't have to re-query DataStore. */
    private var hasHosts = false

    /** User dismissed the onboarding card this session — don't re-show until the
     *  process restarts (state changes still won't resurrect it). */
    private var onboardDismissed = false

    private val scanLauncher =
        registerForActivityResult(androidx.activity.result.contract.ActivityResultContracts.StartActivityForResult()) { result ->
            val raw = result.data?.getStringExtra(QrScanActivity.EXTRA_RESULT) ?: return@registerForActivityResult
            handlePairingUri(raw)
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivityHostListBinding.inflate(layoutInflater)
        setContentView(binding.root)
        store = HostStore(applicationContext)

        binding.list.layoutManager = LinearLayoutManager(this)
        binding.list.adapter = adapter
        binding.scanButton.setOnClickListener {
            scanLauncher.launch(Intent(this, QrScanActivity::class.java))
        }
        binding.addButton.setOnClickListener { showAddDialog() }

        binding.onboardDismiss.setOnClickListener {
            onboardDismissed = true
            binding.onboardCard.visibility = View.GONE
        }

        lifecycleScope.launch {
            store.hosts.collectLatest { hosts ->
                adapter.submit(hosts)
                hasHosts = hosts.isNotEmpty()
                binding.empty.visibility = if (hosts.isEmpty()) View.VISIBLE else View.GONE
                refreshOnboarding()
            }
        }

        handleDeepLink(intent)
    }

    override fun onResume() {
        super.onResume()
        // Re-evaluate when the user returns from installing/enabling Tailscale.
        refreshOnboarding()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleDeepLink(intent)
    }

    /**
     * Proactive launch-time onboarding (distinct from the connect-time
     * [showTailscaleNeeded] dialog). Picks the first unmet prerequisite and shows
     * a dismissible card. Re-runs from onCreate's flow collector and onResume.
     *
     * State priority:
     *   a) Tailscale not installed   → "Install Tailscale"
     *   b) Installed, VPN not active → "Turn on Tailscale"
     *   c) VPN active, no saved hosts → "Pair your PC" (Scan QR)
     *   d) Has hosts + VPN active     → no card
     */
    private fun refreshOnboarding() {
        if (onboardDismissed) {
            binding.onboardCard.visibility = View.GONE
            return
        }

        val installed = isTailscaleInstalled()
        val vpnActive = VpnState.hasActiveVpn(this)

        val state: OnboardState? = when {
            !installed -> OnboardState.Install
            !vpnActive -> OnboardState.Enable
            !hasHosts -> OnboardState.Pair
            else -> null
        }

        if (state == null) {
            binding.onboardCard.visibility = View.GONE
            return
        }

        binding.onboardTitle.setText(state.titleRes)
        binding.onboardBody.text =
            HtmlCompat.fromHtml(getString(state.bodyRes), HtmlCompat.FROM_HTML_MODE_LEGACY)
        binding.onboardBody.movementMethod = LinkMovementMethod.getInstance()
        binding.onboardAction.setText(state.actionRes)
        binding.onboardAction.setOnClickListener {
            when (state) {
                OnboardState.Install -> openTailscale()
                OnboardState.Enable -> launchTailscale()
                OnboardState.Pair ->
                    scanLauncher.launch(Intent(this, QrScanActivity::class.java))
            }
        }
        binding.onboardCard.visibility = View.VISIBLE
    }

    private enum class OnboardState(val titleRes: Int, val bodyRes: Int, val actionRes: Int) {
        Install(R.string.onboard_install_title, R.string.onboard_install_body, R.string.onboard_install_action),
        Enable(R.string.onboard_enable_title, R.string.onboard_enable_body, R.string.onboard_enable_action),
        Pair(R.string.onboard_pair_title, R.string.onboard_pair_body, R.string.onboard_pair_action),
    }

    /** Reliable install probe — needs the <queries> entry in the manifest so
     *  API 30+ doesn't throw NameNotFound spuriously. */
    private fun isTailscaleInstalled(): Boolean =
        try {
            packageManager.getPackageInfo(TAILSCALE_PKG, 0)
            true
        } catch (e: PackageManager.NameNotFoundException) {
            false
        }

    /** Open the installed Tailscale app so the user can turn the VPN on; fall
     *  back to its store page if the launch intent is unexpectedly null. */
    private fun launchTailscale() {
        val launch = packageManager.getLaunchIntentForPackage(TAILSCALE_PKG)
        if (launch != null) {
            startActivity(launch)
        } else {
            openTailscale()
        }
    }

    /** urc:// VIEW intent → save + connect with the same payload the QR uses. */
    private fun handleDeepLink(intent: Intent?) {
        if (intent?.action != Intent.ACTION_VIEW) return
        val data: Uri = intent.data ?: return
        handlePairingUri(data.toString())
    }

    private fun handlePairingUri(raw: String) {
        val host = UrcUri.parse(raw)
        if (host == null) {
            Toast.makeText(this, "Not a valid urc:// pairing link", Toast.LENGTH_LONG).show()
            return
        }
        lifecycleScope.launch {
            store.add(host)
            connect(host)
        }
    }

    private fun connect(host: Host) {
        if (!host.isTailnet) {
            Toast.makeText(this, getString(R.string.err_not_cgnat, host.host), Toast.LENGTH_LONG).show()
            return
        }
        // Non-cancelable: the connect coroutine always reaches progress.dismiss();
        // a user-dismissable dialog could be dismissed twice / out of order.
        val progress = AlertDialog.Builder(this)
            .setMessage(R.string.connecting)
            .setCancelable(false)
            .show()
        lifecycleScope.launch {
            val verdict = Connector.decide(this@HostListActivity, host)
            progress.dismiss()
            when (verdict) {
                is Connector.Verdict.Connect ->
                    startActivity(MainActivity.intent(this@HostListActivity, host.host, host.port, host.displayName))

                is Connector.Verdict.NotTailnet ->
                    Toast.makeText(this@HostListActivity, getString(R.string.err_not_cgnat, host.host), Toast.LENGTH_LONG).show()

                is Connector.Verdict.Unreachable ->
                    showTailscaleNeeded(host, verdict)
            }
        }
    }

    private fun showTailscaleNeeded(host: Host, verdict: Connector.Verdict.Unreachable) {
        val body = if (verdict.vpnLikelyOff) {
            getString(R.string.tailscale_needed_body)
        } else {
            getString(R.string.err_unreachable, host.displayName)
        }
        AlertDialog.Builder(this)
            .setTitle(R.string.tailscale_needed)
            .setMessage(body)
            .setPositiveButton(R.string.retry) { _, _ -> connect(host) }
            .setNeutralButton(R.string.open_tailscale) { _, _ -> openTailscale() }
            .setNegativeButton(R.string.cancel, null)
            .show()
    }

    private fun openTailscale() {
        // Deep-link to the Tailscale app in the Play Store; fall through to a web
        // URL if no store app handles market:// .
        try {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("market://details?id=$TAILSCALE_PKG")))
        } catch (e: Exception) {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("https://play.google.com/store/apps/details?id=$TAILSCALE_PKG")))
        }
    }

    private fun showAddDialog() {
        val dialogBinding = DialogAddHostBinding.inflate(layoutInflater)
        dialogBinding.port.setText(DEFAULT_WEB_TLS_PORT.toString())
        AlertDialog.Builder(this)
            .setTitle(R.string.add_manually)
            .setView(dialogBinding.root)
            .setPositiveButton(R.string.add) { _, _ ->
                val ip = dialogBinding.host.text?.toString()?.trim().orEmpty()
                // SECURITY: require a numeric IPv4 literal — a hostname here would
                // be a DNS-rebinding vector (see Cgnat). The MagicDNS name goes in
                // its own display-only field.
                if (Cgnat.parseIpv4Literal(ip) == null) {
                    Toast.makeText(this, R.string.err_not_ip_literal, Toast.LENGTH_LONG).show()
                    return@setPositiveButton
                }
                val port = dialogBinding.port.text?.toString()?.toIntOrNull() ?: DEFAULT_WEB_TLS_PORT
                val host = Host(
                    host = ip,
                    port = port,
                    magicDns = dialogBinding.magicdns.text?.toString()?.trim()?.ifBlank { null },
                    name = dialogBinding.name.text?.toString()?.trim()?.ifBlank { null },
                )
                lifecycleScope.launch { store.add(host) }
            }
            .setNegativeButton(R.string.cancel, null)
            .show()
    }

    private fun confirmDelete(host: Host) {
        AlertDialog.Builder(this)
            .setMessage("Remove ${host.displayName}?")
            .setPositiveButton(R.string.delete) { _, _ -> lifecycleScope.launch { store.remove(host) } }
            .setNegativeButton(R.string.cancel, null)
            .show()
    }

    private class HostAdapter(
        val onClick: (Host) -> Unit,
        val onLongClick: (Host) -> Unit,
    ) : RecyclerView.Adapter<HostAdapter.VH>() {

        private val items = mutableListOf<Host>()

        fun submit(list: List<Host>) {
            items.clear()
            items.addAll(list)
            notifyDataSetChanged()
        }

        class VH(val binding: ItemHostBinding) : RecyclerView.ViewHolder(binding.root)

        override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): VH =
            VH(ItemHostBinding.inflate(LayoutInflater.from(parent.context), parent, false))

        override fun getItemCount() = items.size

        override fun onBindViewHolder(holder: VH, position: Int) {
            val host = items[position]
            holder.binding.name.text = host.displayName
            holder.binding.detail.text = "${host.host}:${host.port}"
            holder.binding.root.setOnClickListener { onClick(host) }
            holder.binding.root.setOnLongClickListener { onLongClick(host); true }
        }
    }

    private companion object {
        const val TAILSCALE_PKG = "com.tailscale.ipn"
    }
}

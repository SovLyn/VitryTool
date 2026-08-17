package com.sovly.vitrytool

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Bundle
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  private var multicastLock: WifiManager.MulticastLock? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    acquireMulticastLock()
  }

  override fun onDestroy() {
    releaseMulticastLock()
    super.onDestroy()
  }

  /**
   * 持有 WifiManager.MulticastLock：Android 默认丢弃组播包，libp2p-mdns 发现
   * 需要持锁才能收到 mDNS 广播（契约 docs/api/mobile.md 5.4）。
   * 无 WiFi（如纯蜂窝网络）时忽略：mDNS 不可用但其余功能不受影响。
   */
  private fun acquireMulticastLock() {
    try {
      val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
      multicastLock = wifi.createMulticastLock("vitrytool-lan-mdns").apply {
        setReferenceCounted(false)
        acquire()
      }
    } catch (_: Exception) {
      // 忽略：无 WiFi 环境
    }
  }

  private fun releaseMulticastLock() {
    try {
      multicastLock?.release()
    } catch (_: Exception) {
      // 忽略释放失败
    }
    multicastLock = null
  }
}

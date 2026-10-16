import {
  RestartAltRounded,
  SwitchAccessShortcutRounded,
} from '@mui/icons-material'
import {
  Box,
  Button,
  CircularProgress,
  List,
  ListItemButton,
  ListItemText,
  Tooltip,
} from '@mui/material'
import { useLockFn } from 'ahooks'
import type { Ref } from 'react'
import { useImperativeHandle, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { closeAllConnections } from 'tauri-plugin-mihomo-api'

import { BaseDialog, DialogRef } from '@/components/base'
import { useClash, useClashInfo } from '@/hooks/use-clash'
import { useVerge } from '@/hooks/use-verge'
import { changeClashCore, restartCore, upgradeClashCore } from '@/services/cmds'
import { mutate } from '@/services/mutate'
import { showNotice } from '@/services/notice-service'

const VALID_CORE = [
  {
    core: 'verge-mihomo',
    labelKey: 'settings.modals.clashCore.variants.release',
  },
  {
    core: 'verge-mihomo-alpha',
    labelKey: 'settings.modals.clashCore.variants.alpha',
  },
]

export function ClashCoreViewer({ ref }: { ref?: Ref<DialogRef> }) {
  const { t } = useTranslation()

  const { verge, mutateVerge } = useVerge()
  const { version, mutateVersion } = useClash()
  const { invalidateClashConfig } = useClashInfo()

  const [open, setOpen] = useState(false)
  const [upgrading, setUpgrading] = useState(false)
  const [restarting, setRestarting] = useState(false)
  const [changingCore, setChangingCore] = useState<string | null>(null)

  useImperativeHandle(ref, () => ({
    open: () => setOpen(true),
    close: () => setOpen(false),
  }))

  const { clash_core = 'verge-mihomo' } = verge ?? {}
  const customCore = version.includes('verge-adaptive')

  const onCoreChange = useLockFn(async (core: string) => {
    if (core === clash_core) return

    try {
      setChangingCore(core)
      void mutate(() => closeAllConnections(), {
        id: 'close-all-connections',
        errorNotice: false,
      })
      const result = await mutate(() => changeClashCore(core), {
        id: 'change-clash-core',
        errorNotice: false,
      })
      if (!result.ok) return
      const errorMsg = result.value

      if (errorMsg) {
        showNotice.error(errorMsg)
        setChangingCore(null)
        return
      }

      mutateVerge()
      await new Promise((resolve) => setTimeout(resolve, 500))
      invalidateClashConfig()
      mutateVersion()
    } catch (err) {
      showNotice.error(err)
    } finally {
      setChangingCore(null)
    }
  })

  const onRestart = useLockFn(async () => {
    try {
      setRestarting(true)
      await mutate(() => restartCore(), {
        id: 'restart-core',
        successNotice: t(
          'settings.feedback.notifications.clash.restartSuccess',
        ),
      })
      setRestarting(false)
    } catch {
      setRestarting(false)
    }
  })

  const onUpgrade = useLockFn(async () => {
    try {
      setUpgrading(true)
      await mutate(() => upgradeClashCore(), {
        id: 'upgrade-clash-core',
        onFulfilled: (report) => {
          showNotice.success(
            report.upgraded
              ? t('settings.feedback.notifications.clash.versionUpdated')
              : t('settings.feedback.notifications.clash.alreadyLatestVersion'),
          )
        },
      })
    } catch {
      // the funnel already notified
    } finally {
      setUpgrading(false)
    }
  })

  return (
    <BaseDialog
      open={open}
      title={
        <Box sx={{ display: 'flex', justifyContent: 'space-between' }}>
          {t('settings.sections.clash.form.fields.clashCore')}
          <Box>
            <Tooltip
              title={
                customCore
                  ? t('settings.modals.clashCore.upgradeUnavailable')
                  : ''
              }
            >
              <span>
                <Button
                  variant="contained"
                  size="small"
                  startIcon={<SwitchAccessShortcutRounded />}
                  loadingPosition="start"
                  loading={upgrading}
                  disabled={customCore || restarting || changingCore !== null}
                  sx={{ marginRight: '8px' }}
                  onClick={onUpgrade}
                >
                  {t('shared.actions.upgrade')}
                </Button>
              </span>
            </Tooltip>
            <Button
              variant="contained"
              size="small"
              startIcon={<RestartAltRounded />}
              loadingPosition="start"
              loading={restarting}
              disabled={upgrading}
              onClick={onRestart}
            >
              {t('shared.actions.restart')}
            </Button>
          </Box>
        </Box>
      }
      contentSx={{
        pb: 0,
        width: 400,
        height: 180,
        overflowY: 'auto',
        userSelect: 'text',
        marginTop: '-8px',
      }}
      disableOk
      cancelBtn={t('shared.actions.close')}
      onClose={() => setOpen(false)}
      onCancel={() => setOpen(false)}
    >
      <List component="nav">
        {VALID_CORE.map((each) => (
          <ListItemButton
            key={each.core}
            selected={each.core === clash_core}
            onClick={() => onCoreChange(each.core)}
            disabled={changingCore !== null || restarting || upgrading}
          >
            <ListItemText
              primary={`Mihomo ${t(each.labelKey)}`}
              secondary={`/${each.core}`}
            />
            {changingCore === each.core && (
              <CircularProgress size={20} sx={{ mr: 1 }} />
            )}
          </ListItemButton>
        ))}
      </List>
    </BaseDialog>
  )
}

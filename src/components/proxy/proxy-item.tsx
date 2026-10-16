import { CheckCircleOutlineRounded } from '@mui/icons-material'
import {
  alpha,
  Box,
  ListItem,
  ListItemButton,
  ListItemIcon,
  ListItemText,
  styled,
  Tooltip,
  type SxProps,
  type Theme,
} from '@mui/material'
import { useTranslation } from 'react-i18next'

import { BaseLoading } from '@/components/base'
import { useProxyDelayState } from '@/hooks/use-proxy-delay-state'
import delayManager from '@/services/delay'
import {
  memberDetails,
  type ProxyGroupView,
  type ResolvedProxyMember,
} from '@/types/proxy-view'

import { candidateActivity } from './enhancement-status'

interface Props {
  group: ProxyGroupView
  member: ResolvedProxyMember
  selected: boolean
  showType?: boolean
  sx?: SxProps<Theme>
  onClick?: (member: ResolvedProxyMember) => void
}

const Widget = styled(Box)(() => ({
  padding: '3px 6px',
  fontSize: 14,
  borderRadius: '4px',
}))

const TypeBox = styled('span')(({ theme }) => ({
  display: 'inline-block',
  border: '1px solid #ccc',
  borderColor: alpha(theme.palette.text.secondary, 0.36),
  color: alpha(theme.palette.text.secondary, 0.42),
  borderRadius: 4,
  fontSize: 10,
  marginRight: '4px',
  padding: '0 2px',
  lineHeight: 1.25,
}))

export const ProxyEnhancementBadge = ({
  group,
  name,
}: {
  group: ProxyGroupView
  name: string
}) => {
  const { t } = useTranslation()
  if (!group.enhancement || group.enhancement.state === 'unsupported') {
    return null
  }
  const candidate = group.enhancement.candidates.find(
    (candidate) => candidate.name === name,
  )
  const state = candidate?.state ?? 'untested'
  const labels: Record<string, string> = {
    probing: t('proxies.page.enhancement.candidates.probing'),
    trial: t('proxies.page.enhancement.candidates.trial'),
    selected: t('proxies.page.enhancement.candidates.selected'),
    cooldown: t('proxies.page.enhancement.candidates.cooldown'),
    standby: t('proxies.page.enhancement.candidates.standby'),
    untested: t('proxies.page.enhancement.candidates.untested'),
    unavailable: t('proxies.page.enhancement.candidates.unavailable'),
  }
  const label = labels[state] ?? labels.untested
  const cooldown = candidate?.cooldownRemainingSeconds
  const cooldownLabel =
    state === 'cooldown' && cooldown != null
      ? t('proxies.page.enhancement.retryRemaining', { seconds: cooldown })
      : ''
  const activity = candidateActivity(candidate)
  const activityText =
    activity === 'score'
      ? t('proxies.page.enhancement.score', {
          score: candidate!.score!.toFixed(0),
        })
      : activity === 'keepObserving'
        ? t('proxies.page.enhancement.keepObserving')
        : activity === 'observing'
          ? t('proxies.page.enhancement.insufficientEvidence')
          : ''
  const statusText = [label, cooldownLabel].filter(Boolean).join(' · ')
  const lastChecked = candidate?.lastCheckedSecondsAgo
  const checkRecency =
    lastChecked != null
      ? t('proxies.page.enhancement.checkRecency', { seconds: lastChecked })
      : t('proxies.page.enhancement.notChecked')
  const detailText = [statusText, activityText, checkRecency]
    .filter(Boolean)
    .join(' · ')
  return (
    <Tooltip title={detailText}>
      <Box
        component="span"
        sx={{
          display: 'inline-block',
          color: state === 'selected' ? 'primary.main' : 'text.secondary',
          fontSize: 10,
          whiteSpace: 'nowrap',
          mr: 0.5,
        }}
      >
        {statusText}
      </Box>
    </Tooltip>
  )
}

export const ProxyItem = (props: Props) => {
  const { t } = useTranslation()
  const { group, member, selected, showType = true, sx, onClick } = props
  const details = memberDetails(member)
  const unresolved = member.kind === 'unresolved'
  const name = member.ref.name
  const type = unresolved ? member.ref.reason : (details?.type ?? '')
  const now = member.kind === 'group' ? member.group.now : undefined

  // -1/<=0 为不显示，-2 为 loading
  const { delayValue, isPreset, timeout, onDelay } = useProxyDelayState(
    member,
    group.name,
  )

  return (
    <ListItem sx={sx}>
      <ListItemButton
        dense
        disabled={unresolved}
        selected={!unresolved && selected}
        onClick={unresolved ? undefined : () => onClick?.(member)}
        sx={[
          { borderRadius: 1 },
          ({ palette: { mode, primary } }) => {
            const bgcolor = mode === 'light' ? '#ffffff' : '#24252f'
            const selectColor = mode === 'light' ? primary.main : primary.light
            const showDelay = delayValue > 0

            return {
              '&:hover .the-check': { display: !showDelay ? 'block' : 'none' },
              '&:hover .the-delay': { display: showDelay ? 'block' : 'none' },
              '&:hover .the-icon': { display: 'none' },
              '&.Mui-selected': {
                width: `calc(100% + 3px)`,
                marginLeft: `-3px`,
                borderLeft: `3px solid ${selectColor}`,
                bgcolor:
                  mode === 'light'
                    ? alpha(primary.main, 0.15)
                    : alpha(primary.main, 0.35),
              },
              backgroundColor: bgcolor,
              marginBottom: '8px',
              height: '40px',
            }
          },
        ]}
      >
        <ListItemText
          title={name}
          secondary={
            <>
              <Box
                sx={{
                  display: 'inline-block',
                  marginRight: '8px',
                  fontSize: '14px',
                  color: 'text.primary',
                }}
              >
                {name}
                {showType && now && ` - ${now}`}
              </Box>
              <ProxyEnhancementBadge group={group} name={name} />
              {showType && <TypeBox>{type}</TypeBox>}
              {!unresolved && showType && details?.udp && (
                <TypeBox>UDP</TypeBox>
              )}
              {!unresolved && showType && details?.xudp && (
                <TypeBox>XUDP</TypeBox>
              )}
              {!unresolved && showType && details?.tfo && (
                <TypeBox>TFO</TypeBox>
              )}
              {!unresolved && showType && details?.mptcp && (
                <TypeBox>MPTCP</TypeBox>
              )}
              {!unresolved && showType && details?.smux && (
                <TypeBox>SMUX</TypeBox>
              )}
            </>
          }
        />

        <ListItemIcon
          sx={{
            justifyContent: 'flex-end',
            color: 'primary.main',
            display: isPreset ? 'none' : '',
          }}
        >
          {!unresolved && delayValue === -2 && (
            <Widget>
              <BaseLoading />
            </Widget>
          )}

          {!unresolved && delayValue !== -2 && (
            <Widget
              className="the-check"
              onClick={(e) => {
                e.preventDefault()
                e.stopPropagation()
                void onDelay()
              }}
              sx={({ palette }) => ({
                display: 'none', // hover 时显示
                ':hover': { bgcolor: alpha(palette.primary.main, 0.15) },
              })}
            >
              {t('shared.actions.check')}
            </Widget>
          )}

          {!unresolved && delayValue > 0 && (
            // 显示延迟
            <Widget
              className="the-delay"
              onClick={(e) => {
                e.preventDefault()
                e.stopPropagation()
                void onDelay()
              }}
              sx={({ palette }) => ({
                color: delayManager.formatDelayColor(delayValue, timeout),
                ':hover': { bgcolor: alpha(palette.primary.main, 0.15) },
              })}
            >
              {delayManager.formatDelay(delayValue, timeout)}
            </Widget>
          )}

          {!unresolved && delayValue !== -2 && delayValue <= 0 && selected && (
            // 展示已选择的 icon
            <CheckCircleOutlineRounded
              className="the-icon"
              sx={{ fontSize: 16 }}
            />
          )}
        </ListItemIcon>
      </ListItemButton>
    </ListItem>
  )
}

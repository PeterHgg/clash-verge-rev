import {
  Button,
  Dialog,
  type DialogProps,
  DialogActions,
  DialogContent,
  DialogTitle,
  type SxProps,
  type Theme,
} from '@mui/material'
import { ReactNode } from 'react'

interface Props {
  title: ReactNode
  open: boolean
  okBtn?: ReactNode
  cancelBtn?: ReactNode
  disableEnforceFocus?: boolean
  disableOk?: boolean
  disableCancel?: boolean
  disableFooter?: boolean
  contentSx?: SxProps<Theme>
  maxWidth?: DialogProps['maxWidth']
  children?: ReactNode
  loading?: boolean
  onOk?: () => void
  onCancel?: () => void
  onClose?: () => void
}

export interface DialogRef {
  open: () => void
  close: () => void
}

export const BaseDialog: React.FC<Props> = ({
  open,
  title,
  children,
  okBtn,
  cancelBtn,
  disableEnforceFocus,
  contentSx,
  maxWidth,
  disableCancel,
  disableOk,
  disableFooter,
  loading,
  onOk,
  onCancel,
  onClose,
}) => {
  return (
    <Dialog
      open={open}
      maxWidth={maxWidth}
      onClose={onClose}
      disableEnforceFocus={disableEnforceFocus}
    >
      <DialogTitle>{title}</DialogTitle>

      <DialogContent sx={contentSx}>{children}</DialogContent>

      {!disableFooter && (
        <DialogActions>
          {!disableCancel && (
            <Button variant="outlined" onClick={onCancel}>
              {cancelBtn}
            </Button>
          )}
          {!disableOk && (
            <Button loading={loading} variant="contained" onClick={onOk}>
              {okBtn}
            </Button>
          )}
        </DialogActions>
      )}
    </Dialog>
  )
}
